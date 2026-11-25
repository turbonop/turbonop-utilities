// Procupine's view: the processes as a tree (or a list) that updates as
// samples come, the system's graphs above, and the selected process's
// details below. The extension (extension.js) posts it `procupine serve`'s
// events; it posts back what to do (kill, suspend, copy, reveal).
"use strict";

(function () {
  const M = window.ProcupineModel;
  const vscode = typeof acquireVsCodeApi === "function" ? acquireVsCodeApi() : { postMessage() {}, getState() {}, setState() {} };
  const $ = (id) => document.getElementById(id);
  const esc = (s) =>
    String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

  const CHEVRON_RIGHT = '<svg viewBox="0 0 16 16" width="12" height="12" data-twisty="1"><path d="M6 4l4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>';
  const CHEVRON_DOWN = '<svg viewBox="0 0 16 16" width="12" height="12" data-twisty="1"><path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>';

  /** Every column the table can show. */
  const COLUMNS = [
    { key: "name", title: "Process", width: 260 },
    { key: "pid", title: "PID", width: 64, right: true },
    { key: "cpu", title: "CPU", width: 64, right: true, desc: true },
    { key: "cpuHistory", title: "CPU history", width: 96, desc: true },
    { key: "private", title: "Private bytes", width: 96, right: true, desc: true },
    { key: "resident", title: "Working set", width: 96, right: true, desc: true },
    { key: "io", title: "I/O", width: 92, right: true, desc: true },
    { key: "readRate", title: "Read/s", width: 86, right: true, desc: true },
    { key: "writeRate", title: "Write/s", width: 86, right: true, desc: true },
    { key: "threads", title: "Threads", width: 62, right: true, desc: true },
    { key: "handles", title: "Handles", width: 66, right: true, desc: true },
    { key: "user", title: "User", width: 150 },
    { key: "state", title: "State", width: 80 },
    { key: "priority", title: "Priority", width: 62, right: true, desc: true },
    { key: "cpuTime", title: "CPU time", width: 96, right: true, desc: true },
    { key: "start", title: "Started", width: 132, desc: true },
    { key: "virtual", title: "Virtual size", width: 96, right: true, desc: true },
    { key: "pageFaults", title: "Faults/s", width: 72, right: true, desc: true },
    { key: "contextSwitches", title: "Switches/s", width: 80, right: true, desc: true },
    { key: "architecture", title: "Architecture", width: 84 },
    { key: "integrity", title: "Integrity", width: 84 },
    { key: "path", title: "Path", width: 320 },
    { key: "commandLine", title: "Command line", width: 480 },
  ];
  const DEFAULT_SHOWN = ["name", "pid", "cpu", "cpuHistory", "private", "resident", "io", "threads", "handles", "user", "commandLine"];

  const saved = vscode.getState() || {};
  const state = {
    tree: saved.tree !== false,
    sort: saved.sort || null,
    shown: saved.shown || DEFAULT_SHOWN.slice(),
    widths: saved.widths || {},
    collapsed: new Set(saved.collapsed || []),
    hideKernel: saved.hideKernel !== false,
    detailsHeight: saved.detailsHeight || 240,
    filter: "",
    paused: false,
    interval: saved.interval || 1000,
  };
  const save = () =>
    vscode.setState({
      tree: state.tree,
      sort: state.sort,
      shown: state.shown,
      widths: state.widths,
      collapsed: [...state.collapsed],
      hideKernel: state.hideKernel,
      detailsHeight: state.detailsHeight,
      interval: state.interval,
    });

  let model = null;
  let info = { platform: "", user: "", hexcavator: false, serverPid: null };
  /** Whether the note on what an unelevated server can't see was shown. */
  let noted = false;
  let rows = [];
  /** Selected processes, by session ID; `focus` is the one keys move. */
  const selected = new Set();
  let focus = null;
  let anchor = null;
  let rowHeight = 22;
  let scheduled = false;

  // The page.
  document.body.innerHTML = `
    <div id="system"></div>
    <div id="message" hidden></div>
    <div id="toolbar">
      <input id="filter" type="search" placeholder="Filter: name, PID, user, path, command line" spellcheck="false">
      <button id="tree" title="Show processes by parent (T)">Tree</button>
      <button id="pause" title="Pause updates (Space)">Pause</button>
      <select id="interval" title="How often to update">
        <option value="250">0.25 s</option><option value="500">0.5 s</option><option value="1000">1 s</option>
        <option value="2000">2 s</option><option value="5000">5 s</option><option value="10000">10 s</option>
      </select>
      <label id="kernel-label" hidden><input id="kernel" type="checkbox"> Kernel threads</label>
      <span class="gap"></span>
      <button id="kill" title="End the selected processes (Delete)">Kill</button>
      <button id="kill-tree" title="End the selected processes and their descendants (Shift+Delete)">Kill tree</button>
      <button id="suspend" title="Suspend the selected processes">Suspend</button>
      <button id="resume" title="Resume the selected processes">Resume</button>
      <span id="status"></span>
    </div>
    <div id="table" tabindex="0">
      <div id="head"></div>
      <div id="scroll"><div id="spacer"><div id="rows"></div></div><div id="empty" hidden>Waiting for the first sample…</div></div>
    </div>
    <div id="splitter" title="Drag to resize"></div>
    <div id="details"></div>
    <div id="menu" hidden></div>`;

  const table = $("table");
  const scroll = $("scroll");

  function columns() {
    const unix = info.platform !== "win32";
    return state.shown
      .map((k) => COLUMNS.find((c) => c.key === k))
      .filter(Boolean)
      .filter((c) => !(unix && c.key === "integrity"))
      .map((c) => ({ ...c, title: c.key === "handles" && unix ? "Files" : c.title, width: state.widths[c.key] || c.width }));
  }

  function setGrid() {
    const cols = columns();
    document.documentElement.style.setProperty("--cols", cols.map((c) => `${c.width}px`).join(" "));
    const width = cols.reduce((a, c) => a + c.width, 0);
    $("spacer").style.width = `${width}px`;
    $("head").style.width = `${width}px`;
  }

  // The header: sorting, resizing, choosing columns.
  function renderHead() {
    setGrid();
    const sort = state.sort;
    $("head").innerHTML = columns()
      .map((c) => {
        const arrow = sort && sort.key === c.key ? `<span class="arrow">${sort.desc ? "▾" : "▴"}</span>` : "";
        return `<div class="h${c.right ? " r" : ""}" data-key="${c.key}" title="${esc(c.title)}: click to sort">${c.right ? arrow : ""}${esc(c.title)}${c.right ? "" : arrow}<span class="grip" data-grip="${c.key}"></span></div>`;
      })
      .join("");
  }

  $("head").addEventListener("click", (e) => {
    if (e.target.dataset.grip) {
      return;
    }
    const h = e.target.closest(".h");
    if (!h) {
      return;
    }
    const col = COLUMNS.find((c) => c.key === h.dataset.key);
    // First click: the column's natural order (largest first for numbers);
    // second: the other; third: none (by start, as the tree is built).
    const first = !!col.desc;
    if (!state.sort || state.sort.key !== col.key) {
      state.sort = { key: col.key, desc: first };
    } else if (state.sort.desc === first) {
      state.sort = { key: col.key, desc: !first };
    } else {
      state.sort = null;
    }
    save();
    renderHead();
    refresh();
  });

  $("head").addEventListener("mousedown", (e) => {
    const key = e.target.dataset.grip;
    if (!key) {
      return;
    }
    e.preventDefault();
    const startX = e.clientX;
    const startW = columns().find((c) => c.key === key).width;
    const move = (ev) => {
      state.widths[key] = Math.max(36, startW + ev.clientX - startX);
      setGrid();
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      save();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });

  $("head").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    showMenu(
      e.clientX,
      e.clientY,
      COLUMNS.filter((c) => c.key !== "name" && !(info.platform !== "win32" && c.key === "integrity")).map((c) => ({
        label: c.key === "handles" && info.platform !== "win32" ? "Files" : c.title,
        checked: state.shown.includes(c.key),
        run: () => {
          if (state.shown.includes(c.key)) {
            state.shown = state.shown.filter((k) => k !== c.key);
          } else {
            const order = COLUMNS.map((x) => x.key);
            state.shown = order.filter((k) => k === c.key || state.shown.includes(k));
          }
          save();
          renderHead();
          refresh();
        },
      })).concat([{ sep: true }, { label: "Default columns", run: () => ((state.shown = DEFAULT_SHOWN.slice()), (state.widths = {}), save(), renderHead(), refresh()) }])
    );
  });

  // Rows.
  function hidden(p) {
    return state.hideKernel && p.kernel;
  }

  function computeRows() {
    rows = model ? model.rows({ tree: state.tree, sort: state.sort, filter: state.filter, collapsed: state.collapsed, hide: hidden }) : [];
  }

  function cell(c, r) {
    const p = r.p;
    const v = p.v;
    switch (c.key) {
      case "name": {
        const twisty = state.tree && r.kids ? (r.open ? CHEVRON_DOWN : CHEVRON_RIGHT) : "";
        const badges = [];
        if (v.state === "suspended" || v.state === "stopped") {
          badges.push(v.state);
        }
        if (p.elevated && info.platform === "win32" && p.integrity === "High") {
          badges.push("admin");
        }
        return `<div class="c c-name" style="padding-left:${2 + r.depth * 14}px" title="${esc(p.path || p.name)}${p.commandLine ? "\n" + esc(p.commandLine) : ""}"><span class="twisty" data-twisty="${p.id}">${twisty}</span><span class="label">${esc(p.name)}</span>${badges.map((b) => `<span class="badge">${b}</span>`).join("")}</div>`;
      }
      case "cpu": {
        const pct = Math.min(100, (v.cpu || 0) * 100);
        const fill = pct > 0 ? `<span class="fill" style="width:${Math.max(1, pct).toFixed(1)}%"></span>` : "";
        return `<div class="c r bar">${fill}<span class="num">${M.percent(v.cpu)}</span></div>`;
      }
      case "cpuHistory": {
        const values = p.history.cpu.list();
        const w = c.width - 12;
        // (Scaled to the busiest of one processor, or the process's own most.)
        const one = model.system ? 1 / model.system.cpus : 1;
        const max = Math.max(one, p.history.cpu.max());
        return `<div class="c spark"><svg viewBox="0 0 ${w} 18" preserveAspectRatio="none"><polyline class="spark-cpu" points="${M.sparkPoints(values, w, 18, max)}"/></svg></div>`;
      }
      case "pid":
        return `<div class="c r">${p.pid}</div>`;
      case "private":
      case "resident":
      case "virtual":
        return `<div class="c r">${M.bytes(v[c.key])}</div>`;
      case "io":
        return `<div class="c r">${M.rate((v.readRate || 0) + (v.writeRate || 0))}</div>`;
      case "readRate":
      case "writeRate":
        return `<div class="c r">${M.rate(v[c.key])}</div>`;
      case "threads":
      case "handles":
      case "priority":
        return `<div class="c r">${M.count(v[c.key])}</div>`;
      case "pageFaults":
      case "contextSwitches":
        return `<div class="c r">${v[c.key] ? M.count(Math.round(v[c.key])) : ""}</div>`;
      case "cpuTime":
        return `<div class="c r">${M.duration((v.userTime || 0) + (v.kernelTime || 0))}</div>`;
      case "start":
        return `<div class="c">${M.startTime(p.start)}</div>`;
      case "state":
        return `<div class="c">${esc(v.state)}</div>`;
      case "user":
        return `<div class="c" title="${esc(p.user)}">${esc(p.user)}</div>`;
      case "path":
        return `<div class="c" title="${esc(p.path)}">${esc(p.path)}</div>`;
      case "commandLine":
        return `<div class="c" title="${esc(p.commandLine)}">${esc(p.commandLine)}</div>`;
      default:
        return `<div class="c">${esc(p[c.key])}</div>`;
    }
  }

  function rowClass(r) {
    const p = r.p;
    const cls = ["row"];
    if (selected.has(p.id)) {
      cls.push("sel");
    }
    if (model.isExited(p)) {
      cls.push("exited");
    } else if (model.isNew(p)) {
      cls.push("new");
    }
    if (p.v.state === "suspended" || p.v.state === "stopped") {
      cls.push("suspended");
    }
    if (p.kernel) {
      cls.push("kernel");
    }
    if (info.user && p.user && !M.isUser(p.user, info.user)) {
      cls.push("other");
    }
    if (!r.match) {
      cls.push("nomatch");
    }
    return cls.join(" ");
  }

  function renderRows() {
    const cols = columns();
    const top = scroll.scrollTop;
    const height = scroll.clientHeight || 600;
    const first = Math.max(0, Math.floor(top / rowHeight) - 10);
    const last = Math.min(rows.length, Math.ceil((top + height) / rowHeight) + 10);
    $("spacer").style.height = `${rows.length * rowHeight}px`;
    const out = [];
    for (let i = first; i < last; i++) {
      const r = rows[i];
      out.push(`<div class="${rowClass(r)}" data-id="${r.p.id}" data-i="${i}">${cols.map((c) => cell(c, r)).join("")}</div>`);
    }
    const el = $("rows");
    el.style.transform = `translateY(${first * rowHeight}px)`;
    el.innerHTML = out.join("");
    $("empty").hidden = !!model;
    if (model && !rows.length) {
      $("empty").hidden = false;
      $("empty").textContent = state.filter ? `No process matches "${state.filter}".` : "No processes.";
    }
  }

  function renderSystem() {
    if (!model || !model.system) {
      $("system").innerHTML = "";
      return;
    }
    const s = model.system;
    const h = model.systemHistory;
    const graph = (ring, cls) => {
      const values = ring.list();
      const pts = M.sparkPoints(values, 120, 34, 1);
      const area = pts ? `${pts} 120,34 ${(120 - (120 / (M.HISTORY - 1)) * (values.length - 1)).toFixed(1)},34` : "";
      return `<svg viewBox="0 0 120 34" preserveAspectRatio="none"><polygon class="${cls}" points="${area}"/></svg>`;
    };
    const used = s.memoryTotal - s.memoryAvailable;
    let html = `
      <div class="meter">${graph(h.cpu, "line-cpu")}<span class="label">CPU</span><span class="value">${(s.cpu * 100).toFixed(1)}% of ${s.cpus} processors</span></div>
      <div class="meter">${graph(h.memory, "line-memory")}<span class="label">Memory</span><span class="value">${M.bytes(used)} of ${M.bytes(s.memoryTotal)}</span></div>`;
    if (s.commitLimit) {
      html += `<div class="meter">${graph(h.commit, "line-io")}<span class="label">Commit</span><span class="value">${M.bytes(s.commit)} of ${M.bytes(s.commitLimit)}</span></div>`;
    } else if (s.swapTotal) {
      html += `<div class="meter"><svg viewBox="0 0 120 34"></svg><span class="label">Swap</span><span class="value">${M.bytes(s.swapUsed)} of ${M.bytes(s.swapTotal)}</span></div>`;
    }
    html += `<div id="counts"><span>${M.count(s.processes)} processes</span><span>${M.count(s.threads)} threads</span><span>${M.count(s.handles)} ${info.platform === "win32" ? "handles" : "files"}</span><span>up ${uptime(s.uptime)}</span></div>`;
    $("system").innerHTML = html;
  }

  function uptime(ms) {
    const d = Math.floor(ms / 86400000);
    const h = Math.floor((ms % 86400000) / 3600000);
    const m = Math.floor((ms % 3600000) / 60000);
    return d ? `${d}d ${h}h ${m}m` : h ? `${h}h ${m}m` : `${m}m`;
  }

  function chart(ring, cls, max) {
    const values = ring.list();
    const w = 300;
    const pts = M.sparkPoints(values, w, 44, max);
    const x0 = (w - (w / (M.HISTORY - 1)) * (values.length - 1)).toFixed(1);
    return `<svg viewBox="0 0 ${w} 44" preserveAspectRatio="none"><polygon class="${cls}" points="${pts ? `${pts} ${w},44 ${x0},44` : ""}"/></svg>`;
  }

  function renderDetails() {
    const el = $("details");
    const p = focus != null && model ? model.get(focus) : null;
    $("splitter").hidden = !p;
    el.hidden = !p;
    if (!p) {
      return;
    }
    el.style.height = `${state.detailsHeight}px`;
    const v = p.v;
    const parent = p.parent != null ? model.get(p.parent) : null;
    const field = (name, value, acts = "") => (value == null || value === "" ? "" : `<dt>${name}</dt><dd>${value}${acts}</dd>`);
    const act = (name, what) => `<span class="act" data-act="${what}">${name}</span>`;
    const pathActs = p.path ? act("Copy", "copy-path") + act("Reveal", "reveal") + (info.hexcavator ? act("Open in Hexcavator", "hexcavator") : "") : "";
    const cpus = model.system ? model.system.cpus : 1;
    el.innerHTML = `
      <div class="title">${esc(p.name)} <span class="sub">PID ${p.pid}${v.state ? " · " + esc(v.state) : ""}${model.isExited(p) ? " · exited" : ""}</span></div>
      <div class="cols">
        <dl>
          ${field("Path", esc(p.path), pathActs)}
          ${field("Command line", p.commandLine ? `<code>${esc(p.commandLine)}</code>` : "", p.commandLine ? act("Copy", "copy-command") : "")}
          ${field("User", esc(p.user))}
          ${field("Parent", parent ? `${esc(parent.name)} (${parent.pid})` : p.parentPid != null ? `${p.parentPid} (exited)` : "", parent ? act("Select", "parent") : "")}
          ${field("Started", p.start ? esc(new Date(p.start).toLocaleString()) : "")}
          ${field("Architecture", esc(p.architecture))}
          ${field("Integrity", esc(p.integrity))}
          ${field("Elevated", p.elevated == null ? "" : p.elevated ? "yes" : "no")}
          ${field("CPU time", `${M.duration((v.userTime || 0) + (v.kernelTime || 0))} (user ${M.duration(v.userTime)}, kernel ${M.duration(v.kernelTime)})`)}
          ${field("Memory", `private ${M.bytes(v.private)}, working set ${M.bytes(v.resident)}, virtual ${M.bytes(v.virtual)}`)}
          ${field("I/O", v.readBytes == null ? "" : `read ${M.bytes(v.readBytes)}, written ${M.bytes(v.writeBytes)}`)}
          ${field("Threads", M.count(v.threads))}
          ${field(info.platform === "win32" ? "Handles" : "Files", M.count(v.handles))}
          ${field("Priority", M.count(v.priority))}
          ${field("Session", p.session == null ? "" : String(p.session))}
        </dl>
        <div class="charts">
          <div class="chart"><span>CPU</span><span>${M.percent(v.cpu) || "0"}% (peak ${M.percent(p.history.cpu.max()) || "0"}%)</span>${chart(p.history.cpu, "line-cpu", Math.max(1 / cpus, p.history.cpu.max()))}</div>
          <div class="chart"><span>Private bytes</span><span>${M.bytes(v.private)}</span>${chart(p.history.private, "line-memory", p.history.private.max() * 1.1)}</div>
          <div class="chart"><span>I/O</span><span>${M.rate((v.readRate || 0) + (v.writeRate || 0)) || "0 B/s"}</span>${chart(p.history.io, "line-io", p.history.io.max() * 1.1)}</div>
        </div>
      </div>`;
  }

  function renderToolbar() {
    $("tree").classList.toggle("on", state.tree);
    $("pause").textContent = state.paused ? "Resume updates" : "Pause";
    $("pause").classList.toggle("on", state.paused);
    $("interval").value = String(state.interval);
    $("kernel-label").hidden = info.platform !== "linux";
    $("kernel").checked = !state.hideKernel;
    const live = [...selected].map((id) => model && model.get(id)).filter((p) => p && !model.isExited(p));
    for (const id of ["kill", "kill-tree", "suspend", "resume"]) {
      $(id).disabled = !live.length;
    }
    const total = model ? model.processes.size : 0;
    $("status").textContent = model ? `${rows.length === total ? total : `${rows.length} of ${total}`} shown${selected.size > 1 ? ` · ${selected.size} selected` : ""}` : "";
  }

  function refresh() {
    computeRows();
    renderRows();
    renderToolbar();
    renderDetails();
  }

  function schedule() {
    if (!scheduled) {
      scheduled = true;
      requestAnimationFrame(() => {
        scheduled = false;
        renderSystem();
        refresh();
      });
    }
  }

  scroll.addEventListener("scroll", () => {
    $("head").style.transform = `translateX(${-scroll.scrollLeft}px)`;
    renderRows();
  });
  window.addEventListener("resize", () => renderRows());

  // Selection.
  function select(id, { add = false, range = false } = {}) {
    if (range && anchor != null) {
      const a = rows.findIndex((r) => r.p.id === anchor);
      const b = rows.findIndex((r) => r.p.id === id);
      if (a >= 0 && b >= 0) {
        if (!add) {
          selected.clear();
        }
        for (let i = Math.min(a, b); i <= Math.max(a, b); i++) {
          selected.add(rows[i].p.id);
        }
      }
    } else if (add) {
      if (selected.has(id)) {
        selected.delete(id);
      } else {
        selected.add(id);
      }
      anchor = id;
    } else {
      selected.clear();
      selected.add(id);
      anchor = id;
    }
    focus = id;
    refresh();
  }

  function reveal(id) {
    const i = rows.findIndex((r) => r.p.id === id);
    if (i < 0) {
      return;
    }
    const top = i * rowHeight;
    if (top < scroll.scrollTop) {
      scroll.scrollTop = top;
    } else if (top + rowHeight > scroll.scrollTop + scroll.clientHeight) {
      scroll.scrollTop = top + rowHeight - scroll.clientHeight;
    }
    renderRows();
  }

  function toggle(id, open) {
    const isOpen = !state.collapsed.has(id);
    if (open === undefined ? isOpen : !open) {
      state.collapsed.add(id);
    } else {
      state.collapsed.delete(id);
    }
    save();
    refresh();
  }

  $("rows").addEventListener("mousedown", (e) => {
    const rowEl = e.target.closest(".row");
    if (!rowEl) {
      return;
    }
    const id = Number(rowEl.dataset.id);
    if (e.target.closest("[data-twisty]")) {
      toggle(id);
      return;
    }
    if (e.button === 2 && selected.has(id)) {
      focus = id;
      return;
    }
    select(id, { add: e.ctrlKey || e.metaKey, range: e.shiftKey });
  });

  $("rows").addEventListener("dblclick", (e) => {
    const rowEl = e.target.closest(".row");
    if (rowEl && !e.target.closest("[data-twisty]")) {
      toggle(Number(rowEl.dataset.id));
    }
  });

  $("rows").addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const rowEl = e.target.closest(".row");
    if (!rowEl) {
      return;
    }
    processMenu(e.clientX, e.clientY);
  });

  function selectedProcesses() {
    return [...selected].map((id) => model.get(id)).filter((p) => p && !model.isExited(p));
  }

  function act(action) {
    let ps = selectedProcesses();
    if (!ps.length) {
      return;
    }
    if (action === "killTree") {
      const ids = new Set();
      for (const p of ps) {
        for (const id of model.descendants(p.id)) {
          ids.add(id);
        }
      }
      ps = [...ids].map((id) => model.get(id)).filter((p) => p && !model.isExited(p));
    }
    vscode.postMessage({
      type: "action",
      action: action === "killTree" ? "kill" : action,
      tree: action === "killTree",
      processes: ps.map((p) => ({ id: p.id, pid: p.pid, name: p.name })),
    });
  }

  function processMenu(x, y) {
    const ps = selectedProcesses();
    const one = ps.length === 1 ? ps[0] : null;
    const unix = info.platform !== "win32";
    const items = [
      { label: ps.length > 1 ? `Kill ${ps.length} Processes` : "Kill Process", key: "Del", run: () => act("kill"), disabled: !ps.length },
      { label: "Kill Process Tree", key: "Shift+Del", run: () => act("killTree"), disabled: !ps.length },
    ];
    if (unix) {
      items.push({ label: "Terminate (SIGTERM)", run: () => act("terminate"), disabled: !ps.length });
    }
    items.push(
      { label: "Suspend", run: () => act("suspend"), disabled: !ps.length },
      { label: "Resume", run: () => act("resume"), disabled: !ps.length },
      { sep: true },
      { label: "Copy PID", run: () => copy(ps.map((p) => p.pid).join(" ")), disabled: !ps.length },
      { label: "Copy Name", run: () => copy(ps.map((p) => p.name).join("\n")), disabled: !ps.length },
      { label: "Copy Path", run: () => copy(ps.map((p) => p.path || "").join("\n")), disabled: !ps.some((p) => p.path) },
      { label: "Copy Command Line", run: () => copy(ps.map((p) => p.commandLine || "").join("\n")), disabled: !ps.some((p) => p.commandLine) },
      { sep: true },
      { label: "Reveal Executable", run: () => one && vscode.postMessage({ type: "reveal", path: one.path }), disabled: !one || !one.path }
    );
    if (info.hexcavator) {
      items.push({ label: "Open Executable in Hexcavator", run: () => one && vscode.postMessage({ type: "hexcavator", path: one.path }), disabled: !one || !one.path });
    }
    items.push(
      { sep: true },
      { label: "Select Parent", key: "Backspace", run: () => one && selectParent(one), disabled: !one || one.parent == null || !model.get(one.parent) },
      { label: "Expand All", run: () => ((state.collapsed = new Set()), save(), refresh()) },
      { label: "Collapse All", run: () => collapseAll() }
    );
    showMenu(x, y, items);
  }

  function collapseAll() {
    const kids = model.children();
    state.collapsed = new Set([...kids.keys()].filter((k) => k != null));
    save();
    refresh();
  }

  function selectParent(p) {
    if (p.parent != null && model.get(p.parent)) {
      select(p.parent);
      reveal(p.parent);
    }
  }

  function copy(text) {
    vscode.postMessage({ type: "copy", text });
  }

  // Menus.
  let menuItems = [];
  let menuActive = -1;

  function showMenu(x, y, items) {
    menuItems = items;
    menuActive = -1;
    const el = $("menu");
    el.innerHTML = items
      .map((it, i) =>
        it.sep
          ? `<div class="sep"></div>`
          : `<div class="item${it.disabled ? " disabled" : ""}" data-i="${i}"><span>${it.checked !== undefined ? `<span class="check">${it.checked ? "✓" : ""}</span>` : ""}${esc(it.label)}</span><span class="key">${esc(it.key || "")}</span></div>`
      )
      .join("");
    el.hidden = false;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    el.style.left = `${Math.min(x, window.innerWidth - w - 4)}px`;
    el.style.top = `${Math.min(y, window.innerHeight - h - 4)}px`;
  }

  function hideMenu() {
    $("menu").hidden = true;
    menuItems = [];
  }

  $("menu").addEventListener("mousedown", (e) => e.stopPropagation());
  $("menu").addEventListener("click", (e) => {
    const item = e.target.closest(".item");
    if (!item) {
      return;
    }
    const it = menuItems[Number(item.dataset.i)];
    hideMenu();
    if (it && !it.disabled) {
      it.run();
    }
  });
  window.addEventListener("mousedown", () => hideMenu());
  scroll.addEventListener("scroll", () => hideMenu());

  // The keyboard.
  table.addEventListener("keydown", (e) => {
    if (!$("menu").hidden) {
      return;
    }
    const i = rows.findIndex((r) => r.p.id === focus);
    const page = Math.max(1, Math.floor(scroll.clientHeight / rowHeight) - 1);
    const go = (j) => {
      if (!rows.length) {
        return;
      }
      const k = Math.max(0, Math.min(rows.length - 1, j));
      select(rows[k].p.id, { range: e.shiftKey });
      reveal(rows[k].p.id);
    };
    const r = i >= 0 ? rows[i] : null;
    switch (e.key) {
      case "ArrowDown":
        go(i + 1);
        break;
      case "ArrowUp":
        go(i < 0 ? 0 : i - 1);
        break;
      case "PageDown":
        go(i + page);
        break;
      case "PageUp":
        go(i - page);
        break;
      case "Home":
        go(0);
        break;
      case "End":
        go(rows.length - 1);
        break;
      case "ArrowRight":
        if (r && r.kids && !r.open) {
          toggle(r.p.id, true);
        } else if (r && r.kids) {
          go(i + 1);
        }
        break;
      case "ArrowLeft":
        if (r && r.kids && r.open && state.tree) {
          toggle(r.p.id, false);
        } else if (r) {
          selectParent(r.p);
        }
        break;
      case "Backspace":
        if (r) {
          selectParent(r.p);
        }
        break;
      case "Delete":
        act(e.shiftKey ? "killTree" : "kill");
        break;
      case " ":
        togglePause();
        break;
      case "a":
        if (e.ctrlKey || e.metaKey) {
          rows.forEach((x) => selected.add(x.p.id));
          refresh();
        } else {
          return;
        }
        break;
      case "c":
        if ((e.ctrlKey || e.metaKey) && selected.size) {
          copy(selectedProcesses().map((p) => `${p.pid}\t${p.name}\t${p.commandLine || ""}`).join("\n"));
        } else {
          return;
        }
        break;
      case "t":
        toggleTree();
        break;
      case "ContextMenu":
        if (r) {
          const el = document.querySelector(`.row[data-id="${r.p.id}"]`);
          const box = el ? el.getBoundingClientRect() : { left: 40, bottom: 40 };
          processMenu(box.left + 40, box.bottom);
        }
        break;
      default:
        // Typing finds a process by name.
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
          typeAhead(e.key);
          break;
        }
        return;
    }
    e.preventDefault();
  });

  let typed = "";
  let typedAt = 0;
  function typeAhead(ch) {
    const now = Date.now();
    typed = now - typedAt > 800 ? ch : typed + ch;
    typedAt = now;
    const t = typed.toLowerCase();
    const start = Math.max(0, rows.findIndex((r) => r.p.id === focus));
    for (let k = 0; k < rows.length; k++) {
      const r = rows[(start + (typed.length === 1 ? 1 : 0) + k) % rows.length];
      if (r.p.name.toLowerCase().startsWith(t)) {
        select(r.p.id);
        reveal(r.p.id);
        return;
      }
    }
  }

  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === "f") {
      e.preventDefault();
      $("filter").focus();
      $("filter").select();
    } else if (e.key === "Escape") {
      if (!$("menu").hidden) {
        hideMenu();
      } else if (document.activeElement === $("filter")) {
        $("filter").value = "";
        state.filter = "";
        refresh();
        table.focus();
      }
    }
  });

  // The toolbar.
  $("filter").addEventListener("input", () => {
    state.filter = $("filter").value;
    refresh();
  });
  $("filter").addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "Enter") {
      e.preventDefault();
      table.focus();
      if (rows.length && !rows.some((r) => r.p.id === focus)) {
        select(rows[0].p.id);
      }
    }
  });

  function toggleTree() {
    state.tree = !state.tree;
    save();
    refresh();
    if (focus != null) {
      reveal(focus);
    }
  }

  function togglePause() {
    state.paused = !state.paused;
    vscode.postMessage({ type: state.paused ? "pause" : "watch", interval: state.interval });
    renderToolbar();
  }

  $("tree").addEventListener("click", toggleTree);
  $("pause").addEventListener("click", togglePause);
  $("interval").addEventListener("change", () => {
    state.interval = Number($("interval").value);
    save();
    if (!state.paused) {
      vscode.postMessage({ type: "watch", interval: state.interval });
    }
  });
  $("kernel").addEventListener("change", () => {
    state.hideKernel = !$("kernel").checked;
    save();
    refresh();
  });
  $("message").title = "Click to dismiss";
  $("message").addEventListener("click", () => ($("message").hidden = true));
  $("kill").addEventListener("click", () => act("kill"));
  $("kill-tree").addEventListener("click", () => act("killTree"));
  $("suspend").addEventListener("click", () => act("suspend"));
  $("resume").addEventListener("click", () => act("resume"));

  // The details.
  $("details").addEventListener("click", (e) => {
    const what = e.target.dataset.act;
    const p = focus != null && model ? model.get(focus) : null;
    if (!what || !p) {
      return;
    }
    if (what === "copy-path") {
      copy(p.path);
    } else if (what === "copy-command") {
      copy(p.commandLine);
    } else if (what === "reveal") {
      vscode.postMessage({ type: "reveal", path: p.path });
    } else if (what === "hexcavator") {
      vscode.postMessage({ type: "hexcavator", path: p.path });
    } else if (what === "parent") {
      selectParent(p);
    }
  });

  $("splitter").addEventListener("mousedown", (e) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = state.detailsHeight;
    const move = (ev) => {
      state.detailsHeight = Math.max(60, Math.min(window.innerHeight - 160, startH - (ev.clientY - startY)));
      $("details").style.height = `${state.detailsHeight}px`;
      renderRows();
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      save();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });

  // Messages from the extension.
  window.addEventListener("message", (e) => {
    const m = e.data;
    if (!m || typeof m !== "object") {
      return;
    }
    if (m.type === "init") {
      info = { platform: m.platform, user: m.user, hexcavator: !!m.hexcavator, serverPid: m.serverPid };
      noted = false;
      model = new M.Model(m.columns);
      if (m.interval) {
        state.interval = m.interval;
      }
      state.paused = false;
      renderHead();
      schedule();
      vscode.postMessage({ type: "watch", interval: state.interval });
    } else if (m.type === "sample" && model) {
      model.apply(m.event);
      // (The selected that are gone are no longer selected.)
      for (const id of [...selected]) {
        if (!model.get(id)) {
          selected.delete(id);
        }
      }
      if (focus != null && !model.get(focus)) {
        focus = null;
      }
      if (!noted) {
        noteElevation();
      }
      schedule();
    } else if (m.type === "message") {
      $("message").hidden = !m.text;
      $("message").textContent = m.text || "";
    } else if (m.type === "select" && model) {
      const p = [...model.processes.values()].find((x) => x.pid === m.pid && !model.isExited(x));
      if (p) {
        for (const a of model.ancestors(p.id)) {
          state.collapsed.delete(a);
        }
        select(p.id);
        reveal(p.id);
        table.focus();
      }
    }
  });

  /**
   * Without administrator rights (root), other users' processes show
   * less: say so once, from what the server says of itself.
   */
  function noteElevation() {
    const me = [...model.processes.values()].find((p) => p.pid === info.serverPid);
    if (!me) {
      return;
    }
    noted = true;
    if (me.elevated !== false) {
      return;
    }
    const how =
      info.platform === "win32"
        ? "run VS Code as administrator"
        : info.platform === "darwin"
          ? "macOS shows another user's processes only to root"
          : "run VS Code as root";
    const text = `Other users' processes show less (no user, command line, or I/O for some), and can't be killed or suspended: ${how} to see and act on all of them.`;
    $("message").hidden = false;
    $("message").textContent = text;
  }

  // Row height: the theme's font may make rows taller than 22 px.
  const probe = document.createElement("div");
  probe.className = "row";
  probe.style.visibility = "hidden";
  probe.textContent = "x";
  $("rows").appendChild(probe);
  rowHeight = probe.getBoundingClientRect().height || 22;
  probe.remove();

  renderHead();
  renderToolbar();
  renderDetails();
  table.focus();
  vscode.postMessage({ type: "ready" });
})();
