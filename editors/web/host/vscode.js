// The part of VS Code's extension API the Hexcavator extension uses,
// for the web version: the extension's own code (extension.js, sidebar.js,
// and its views) runs unchanged on it, in the page.
//
// - The sidebar's tree view is drawn in #sidebar, with its title bar's
//   buttons and its items' inline buttons and context menus from the
//   extension's package.json (`contributes.menus`, their `when` clauses).
// - Webview panels are tabs in #editors, each an iframe of the panel's
//   HTML, with acquireVsCodeApi, VS Code's theme variables, and webview
//   context menus (`data-vscode-context`, `webview/context`).
// - Text documents (a content provider's) are tabs of text; a diff, two
//   side by side.
// - Quick picks, input boxes, notifications, progress, the status bar, the
//   output channel, settings (the extension's, kept in localStorage), and
//   file dialogs (opening picks files; saving downloads).
"use strict";

(function () {
  const h = (tag, attrs, ...children) => {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === undefined || v === null || v === false) {
        continue;
      }
      if (k === "class") {
        el.className = v;
      } else if (k.startsWith("on")) {
        el.addEventListener(k.slice(2), v);
      } else {
        el.setAttribute(k, v === true ? "" : v);
      }
    }
    for (const c of children.flat()) {
      if (c !== null && c !== undefined && c !== false) {
        el.append(c instanceof Node ? c : document.createTextNode(String(c)));
      }
    }
    return el;
  };

  // ------------------------------------------------------------ basics

  class Disposable {
    constructor(fn) {
      this.fn = fn;
    }
    dispose() {
      if (this.fn) {
        this.fn();
        this.fn = null;
      }
    }
    static from(...ds) {
      return new Disposable(() => ds.forEach((d) => d && d.dispose && d.dispose()));
    }
  }

  class EventEmitter {
    constructor() {
      this.listeners = new Set();
      this.event = (fn, thisArg, subs) => {
        const f = thisArg ? fn.bind(thisArg) : fn;
        this.listeners.add(f);
        const d = new Disposable(() => this.listeners.delete(f));
        if (Array.isArray(subs)) {
          subs.push(d);
        }
        return d;
      };
    }
    fire(v) {
      for (const f of [...this.listeners]) {
        try {
          f(v);
        } catch (e) {
          console.error(e);
        }
      }
    }
    dispose() {
      this.listeners.clear();
    }
  }

  class Uri {
    constructor(scheme, authority, path, query, fragment) {
      this.scheme = scheme || "file";
      this.authority = authority || "";
      this.path = path || "/";
      this.query = query || "";
      this.fragment = fragment || "";
    }
    get fsPath() {
      return this.path;
    }
    with(c) {
      return new Uri(c.scheme ?? this.scheme, c.authority ?? this.authority, c.path ?? this.path, c.query ?? this.query, c.fragment ?? this.fragment);
    }
    toString() {
      const q = this.query ? `?${this.query}` : "";
      const f = this.fragment ? `#${this.fragment}` : "";
      return `${this.scheme}://${this.authority}${this.path}${q}${f}`;
    }
    toJSON() {
      return { scheme: this.scheme, path: this.path, query: this.query };
    }
    static file(p) {
      return new Uri("file", "", String(p).replace(/\\/g, "/"));
    }
    static from(c) {
      return new Uri(c.scheme, c.authority, c.path, c.query, c.fragment);
    }
    static parse(s) {
      const m = /^([a-z][\w+.-]*):(?:\/\/([^/?#]*))?([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/i.exec(String(s));
      return m ? new Uri(m[1], m[2], m[3], m[4], m[5]) : Uri.file(s);
    }
    static joinPath(base, ...parts) {
      return base.with({ path: [base.path.replace(/\/$/, ""), ...parts].join("/") });
    }
  }

  class CancellationTokenSource {
    constructor() {
      const changed = new EventEmitter();
      this.token = { isCancellationRequested: false, onCancellationRequested: changed.event };
      this.changed = changed;
    }
    cancel() {
      if (!this.token.isCancellationRequested) {
        this.token.isCancellationRequested = true;
        this.changed.fire();
      }
    }
    dispose() {}
  }

  class MarkdownString {
    constructor(value) {
      this.value = value || "";
    }
    appendText(t) {
      this.value += String(t).replace(/[\\`*_{}[\]()#+\-.!<>]/g, "\\$&");
      return this;
    }
    appendMarkdown(t) {
      this.value += t;
      return this;
    }
    appendCodeblock(code) {
      this.value += "\n```\n" + code + "\n```\n";
      return this;
    }
  }

  class TreeItem {
    constructor(label, collapsibleState) {
      this.label = label;
      this.collapsibleState = collapsibleState || 0;
    }
  }
  class ThemeIcon {
    constructor(id, color) {
      this.id = id;
      this.color = color;
    }
  }
  ThemeIcon.File = new ThemeIcon("file");
  ThemeIcon.Folder = new ThemeIcon("folder");
  class ThemeColor {
    constructor(id) {
      this.id = id;
    }
  }
  class Position {
    constructor(line, character) {
      this.line = line;
      this.character = character;
    }
  }
  class Range {
    constructor(a, b, c, d) {
      if (a instanceof Position) {
        this.start = a;
        this.end = b;
      } else {
        this.start = new Position(a, b);
        this.end = new Position(c, d);
      }
    }
  }
  class Selection extends Range {}
  class CodeLens {
    constructor(range, command) {
      this.range = range;
      this.command = command;
    }
  }
  class RelativePattern {
    constructor(base, pattern) {
      this.base = base;
      this.pattern = pattern;
    }
  }

  // ------------------------------------------------------------ the extension's manifest

  let manifest = { contributes: {} };
  const commandTitle = (id) => ((manifest.contributes.commands || []).find((c) => c.command === id) || {}).title || id;
  const commandIcon = (id) => {
    const icon = ((manifest.contributes.commands || []).find((c) => c.command === id) || {}).icon;
    const m = typeof icon === "string" && /^\$\(([\w-]+)\)$/.exec(icon);
    return m ? m[1] : null;
  };

  /** Commands the web version can't run (a terminal, the OS's file manager). */
  const UNSUPPORTED = new Set(["hexcavator.runInTerminal", "hexcavator.revealFile", "hexcavator.openInPane"]);

  /** VS Code's `when` clauses, as the extension's menus write them. */
  function when(expr, ctx) {
    if (!expr) {
      return true;
    }
    const tokens = [];
    const re = /\s*(&&|\|\||==|!=|=~|!|\(|\)|'[^']*'|\/(?:\\.|[^/])+\/[a-z]*|[\w.-]+)/gy;
    let m;
    while ((m = re.exec(expr))) {
      tokens.push(m[1]);
    }
    let i = 0;
    const value = (t) => {
      if (t === undefined) {
        return undefined;
      }
      if (t.startsWith("'")) {
        return t.slice(1, -1);
      }
      if (t === "true") {
        return true;
      }
      if (t === "false") {
        return false;
      }
      return ctx[t];
    };
    const primary = () => {
      const t = tokens[i++];
      if (t === "!") {
        return !primary();
      }
      if (t === "(") {
        const v = or();
        i++; // )
        return v;
      }
      const left = value(t);
      const op = tokens[i];
      if (op === "==" || op === "!=") {
        i++;
        const r = tokens[i++];
        const right = r.startsWith("'") ? r.slice(1, -1) : r;
        const eq = String(left) === String(right);
        return op === "==" ? eq : !eq;
      }
      if (op === "=~") {
        i++;
        const r = tokens[i++];
        const mm = /^\/(.*)\/([a-z]*)$/.exec(r);
        return mm ? new RegExp(mm[1], mm[2]).test(String(left ?? "")) : false;
      }
      return !!left;
    };
    const and = () => {
      let v = primary();
      while (tokens[i] === "&&") {
        i++;
        const r = primary();
        v = v && r;
      }
      return v;
    };
    const or = () => {
      let v = and();
      while (tokens[i] === "||") {
        i++;
        const r = and();
        v = v || r;
      }
      return v;
    };
    try {
      return or();
    } catch {
      return false;
    }
  }

  /** The menu's entries whose `when` holds, by group (`inline` apart). */
  function menu(id, ctx) {
    return ((manifest.contributes.menus || {})[id] || []).filter((e) => !UNSUPPORTED.has(e.command) && when(e.when, ctx));
  }

  // ------------------------------------------------------------ commands

  const commands = new Map();
  const builtins = {
    "vscode.diff": (left, right, title) => showDiff(left, right, title),
    "workbench.action.openSettings": () => openSettings(),
    "revealFileInOS": () => {},
    "setContext": () => {},
  };
  const commandsApi = {
    registerCommand(id, fn, thisArg) {
      commands.set(id, thisArg ? fn.bind(thisArg) : fn);
      return new Disposable(() => commands.delete(id));
    },
    async executeCommand(id, ...args) {
      if (/\.focus$/.test(id)) {
        return;
      }
      const f = commands.get(id) || builtins[id];
      if (!f) {
        console.warn(`no command ${id}`);
        return;
      }
      return f(...args);
    },
    getCommands: async () => [...commands.keys()],
  };

  /** A `command:` link (the welcome view's, a tooltip's). */
  function runLink(href) {
    const m = /^command:([\w.-]+)(?:\?(.*))?$/.exec(href);
    if (!m) {
      return false;
    }
    let args = [];
    if (m[2]) {
      try {
        args = JSON.parse(decodeURIComponent(m[2]));
      } catch {
        args = [];
      }
    }
    commandsApi.executeCommand(m[1], ...(Array.isArray(args) ? args : [args]));
    return true;
  }

  // ------------------------------------------------------------ settings and state

  const SETTINGS = "hexcavator.web.settings";
  function settings() {
    try {
      return JSON.parse(localStorage.getItem(SETTINGS) || "{}");
    } catch {
      return {};
    }
  }
  const configChanged = new EventEmitter();
  function defaults() {
    const out = {};
    const props = ((manifest.contributes.configuration || {}).properties) || {};
    for (const [k, v] of Object.entries(props)) {
      out[k] = v.default;
    }
    return out;
  }
  function getConfiguration(section) {
    const prefix = section ? `${section}.` : "";
    return {
      get(key, fallback) {
        const full = prefix + key;
        const s = settings();
        if (full in s) {
          return s[full];
        }
        const d = defaults();
        return full in d && d[full] !== undefined ? d[full] : fallback;
      },
      has: (key) => prefix + key in defaults(),
      async update(key, value) {
        const s = settings();
        if (value === undefined) {
          delete s[prefix + key];
        } else {
          s[prefix + key] = value;
        }
        try {
          localStorage.setItem(SETTINGS, JSON.stringify(s));
        } catch {
          // (Kept for this page only.)
        }
        configChanged.fire({ affectsConfiguration: (k) => (prefix + key).startsWith(k) });
      },
    };
  }

  /** A memento kept in memory (files dropped don't outlive the page). */
  function memento() {
    const m = new Map();
    return { get: (k, d) => (m.has(k) ? m.get(k) : d), update: async (k, v) => void m.set(k, v), keys: () => [...m.keys()] };
  }

  // ------------------------------------------------------------ the shell's parts

  const $ = (id) => document.getElementById(id);

  // Tabs (the editor area): each a panel, a text document, or a diff.
  const tabs = [];
  let activeTab = null;
  function addTab(tab) {
    tabs.push(tab);
    tab.head = h(
      "div",
      { class: "tab", role: "tab", title: tab.title, onclick: () => activate(tab), onauxclick: (e) => e.button === 1 && closeTab(tab) },
      h("span", { class: "tab-label" }, tab.title),
      h("span", { class: "tab-close codicon codicon-close", title: "Close", onclick: (e) => (e.stopPropagation(), closeTab(tab)) })
    );
    $("tabs").append(tab.head);
    $("editors").append(tab.body);
    activate(tab);
    $("welcome").hidden = true;
  }
  function activate(tab) {
    const before = activeTab;
    activeTab = tab;
    for (const t of tabs) {
      const on = t === tab;
      t.head.classList.toggle("active", on);
      t.body.hidden = !on;
    }
    if (before && before !== tab && before.onViewState) {
      before.onViewState(false);
    }
    if (tab.onViewState) {
      tab.onViewState(true);
    }
    tab.head.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
  function closeTab(tab) {
    const i = tabs.indexOf(tab);
    if (i < 0) {
      return;
    }
    tabs.splice(i, 1);
    tab.head.remove();
    tab.body.remove();
    if (tab.onClose) {
      tab.onClose();
    }
    if (activeTab === tab) {
      activeTab = null;
      const next = tabs[Math.min(i, tabs.length - 1)];
      if (next) {
        activate(next);
      } else {
        $("welcome").hidden = false;
        status.panel(null);
      }
    }
  }
  function setTabTitle(tab, title) {
    tab.title = title;
    tab.head.title = title;
    tab.head.querySelector(".tab-label").textContent = title;
  }

  // The status bar.
  const status = {
    items: [],
    message: null,
    render() {
      const bar = $("status-right");
      bar.textContent = "";
      if (this.message) {
        bar.append(h("span", { class: "status-item" }, icons(this.message)));
      }
      for (const it of [...this.items].sort((a, b) => b.priority - a.priority)) {
        if (!it.visible || !it.text) {
          continue;
        }
        const tip = it.tooltip && typeof it.tooltip === "object" ? it.tooltip.value : it.tooltip;
        bar.append(
          h(
            "span",
            {
              class: "status-item" + (it.command ? " link" : ""),
              title: tip ? String(tip).replace(/[*`\\]/g, "") : undefined,
              onclick: it.command ? () => commandsApi.executeCommand(typeof it.command === "string" ? it.command : it.command.command) : undefined,
            },
            icons(it.text)
          )
        );
      }
    },
    panel() {},
  };

  /** Text with `$(icon)`s as codicons. */
  function icons(text) {
    const parts = [];
    let last = 0;
    const re = /\$\(([\w-]+)(~spin)?\)/g;
    let m;
    while ((m = re.exec(text))) {
      parts.push(text.slice(last, m.index));
      parts.push(h("span", { class: `codicon codicon-${m[1]}${m[2] ? " codicon-modifier-spin" : ""}` }));
      last = m.index + m[0].length;
    }
    parts.push(text.slice(last));
    return parts;
  }

  // Notifications: messages with their buttons, and progress.
  function notify(kind, message, items) {
    const actions = items.filter((x) => typeof x === "string" || (x && x.title));
    return new Promise((resolve) => {
      const close = (v) => {
        clearTimeout(timer);
        el.remove();
        resolve(v);
      };
      const el = h(
        "div",
        { class: `toast ${kind}`, role: kind === "error" ? "alert" : "status" },
        h("span", { class: `codicon codicon-${kind === "error" ? "error" : kind === "warning" ? "warning" : "info"}` }),
        h("div", { class: "toast-text" }, String(message)),
        h(
          "div",
          { class: "toast-actions" },
          actions.map((a) => h("button", { onclick: () => close(a) }, typeof a === "string" ? a : a.title)),
          h("button", { class: "toast-close codicon codicon-close", title: "Close", onclick: () => close(undefined) })
        )
      );
      $("toasts").append(el);
      const timer = actions.length ? null : setTimeout(() => close(undefined), kind === "error" ? 12000 : 6000);
    });
  }

  async function withProgress(options, task) {
    const source = new CancellationTokenSource();
    const label = h("div", { class: "toast-text" }, options.title || "");
    const el = h(
      "div",
      { class: "toast progress" },
      h("span", { class: "codicon codicon-loading codicon-modifier-spin" }),
      label,
      options.cancellable ? h("div", { class: "toast-actions" }, h("button", { onclick: () => source.cancel() }, "Cancel")) : null
    );
    $("toasts").append(el);
    try {
      return await task(
        {
          report: (p) => {
            if (p && p.message) {
              label.textContent = `${options.title || ""} ${p.message}`;
            }
          },
        },
        source.token
      );
    } finally {
      el.remove();
    }
  }

  // Quick picks and input boxes: a box at the top, with a filter.
  function createQuickPick() {
    const accepted = new EventEmitter();
    const hidden = new EventEmitter();
    const valueChanged = new EventEmitter();
    const activeChanged = new EventEmitter();
    const selectionChanged = new EventEmitter();
    let box = null;
    let active = 0;
    const qp = {
      items: [],
      value: "",
      placeholder: "",
      title: "",
      busy: false,
      matchOnDescription: false,
      matchOnDetail: false,
      canSelectMany: false,
      activeItems: [],
      selectedItems: [],
      onDidAccept: accepted.event,
      onDidHide: hidden.event,
      onDidChangeValue: valueChanged.event,
      onDidChangeActive: activeChanged.event,
      onDidChangeSelection: selectionChanged.event,
      show() {
        if (box) {
          return;
        }
        const input = h("input", { class: "quick-input", type: "text", placeholder: qp.placeholder || "", "aria-label": qp.title || qp.placeholder || "Pick" });
        const list = h("div", { class: "quick-list", role: "listbox" });
        box = h("div", { class: "quick" }, qp.title ? h("div", { class: "quick-title" }, qp.title) : null, input, list);
        const shown = () =>
          qp.items.filter((it) => {
            if (it.kind === -1 || it.alwaysShow) {
              return true;
            }
            const v = qp.value.toLowerCase();
            const hay = [it.label, qp.matchOnDescription ? it.description : "", qp.matchOnDetail ? it.detail : ""].join(" ").toLowerCase();
            return !v || hay.includes(v);
          });
        const draw = () => {
          list.textContent = "";
          const items = shown().filter((it) => it.kind !== -1);
          active = Math.max(0, Math.min(active, items.length - 1));
          qp.activeItems = items[active] ? [items[active]] : [];
          items.forEach((it, i) => {
            list.append(
              h(
                "div",
                {
                  class: "quick-item" + (i === active ? " active" : ""),
                  role: "option",
                  onmousedown: (e) => {
                    e.preventDefault();
                    active = i;
                    qp.activeItems = [it];
                    qp.selectedItems = [it];
                    selectionChanged.fire([it]);
                    accepted.fire();
                  },
                },
                h("span", { class: "quick-label" }, icons(it.label || "")),
                it.description ? h("span", { class: "quick-desc" }, it.description) : null,
                it.detail ? h("div", { class: "quick-detail" }, it.detail) : null
              )
            );
          });
          if (qp.busy) {
            list.append(h("div", { class: "quick-item busy" }, "…"));
          }
        };
        qp.redraw = draw;
        input.addEventListener("input", () => {
          qp.value = input.value;
          active = 0;
          valueChanged.fire(qp.value);
          draw();
        });
        input.addEventListener("keydown", (e) => {
          const items = shown().filter((it) => it.kind !== -1);
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            active = (active + (e.key === "ArrowDown" ? 1 : -1) + items.length) % Math.max(1, items.length);
            qp.activeItems = items[active] ? [items[active]] : [];
            activeChanged.fire(qp.activeItems);
            draw();
          } else if (e.key === "Enter") {
            e.preventDefault();
            qp.selectedItems = qp.activeItems;
            selectionChanged.fire(qp.selectedItems);
            accepted.fire();
          } else if (e.key === "Escape") {
            qp.hide();
          }
        });
        input.addEventListener("blur", () => setTimeout(() => box && !box.contains(document.activeElement) && qp.hide(), 150));
        $("overlay").append(box);
        input.value = qp.value;
        draw();
        input.focus();
      },
      hide() {
        if (box) {
          box.remove();
          box = null;
          hidden.fire();
        }
      },
      dispose() {
        qp.hide();
      },
    };
    // (Assigning items or busy redraws it, as VS Code's does.)
    for (const key of ["items", "busy"]) {
      let v = qp[key];
      Object.defineProperty(qp, key, {
        get: () => v,
        set: (x) => {
          v = x;
          if (qp.redraw && box) {
            qp.redraw();
          }
        },
      });
    }
    return qp;
  }

  async function showQuickPick(items, options) {
    const list = await items;
    const strings = list.map((x) => (typeof x === "string" ? { label: x, value: x } : x));
    return new Promise((resolve) => {
      const qp = createQuickPick();
      qp.title = (options && options.title) || "";
      qp.placeholder = (options && (options.placeHolder || options.placeholder)) || "";
      qp.matchOnDescription = !!(options && options.matchOnDescription);
      qp.items = strings;
      let done = false;
      qp.onDidAccept(() => {
        done = true;
        const it = qp.selectedItems[0] || qp.activeItems[0];
        qp.hide();
        resolve(it ? ("value" in it && typeof list[0] === "string" ? it.value : it) : undefined);
      });
      qp.onDidHide(() => !done && resolve(undefined));
      qp.show();
    });
  }

  async function showInputBox(options) {
    const o = options || {};
    return new Promise((resolve) => {
      const input = h("input", { class: "quick-input", type: o.password ? "password" : "text", placeholder: o.placeHolder || "", value: o.value || "" });
      const message = h("div", { class: "quick-detail" }, o.prompt || "");
      const box = h("div", { class: "quick" }, o.title ? h("div", { class: "quick-title" }, o.title) : null, input, message);
      const close = (v) => {
        box.remove();
        resolve(v);
      };
      input.addEventListener("keydown", async (e) => {
        if (e.key === "Enter") {
          const bad = o.validateInput ? await o.validateInput(input.value) : null;
          if (bad) {
            message.textContent = typeof bad === "string" ? bad : bad.message;
            message.classList.add("error");
          } else {
            close(input.value);
          }
        } else if (e.key === "Escape") {
          close(undefined);
        }
      });
      input.addEventListener("blur", () => setTimeout(() => box.isConnected && close(undefined), 150));
      $("overlay").append(box);
      input.focus();
    });
  }

  // Context menus.
  function contextMenu(x, y, entries, args) {
    document.querySelectorAll(".menu").forEach((m) => m.remove());
    if (!entries.length) {
      return;
    }
    const groups = new Map();
    for (const e of entries) {
      const g = (e.group || "").split("@")[0];
      if (!groups.has(g)) {
        groups.set(g, []);
      }
      groups.get(g).push(e);
    }
    const order = (e) => Number((e.group || "").split("@")[1] || 0);
    const el = h("div", { class: "menu", role: "menu" });
    [...groups.keys()].sort().forEach((g, gi) => {
      if (gi) {
        el.append(h("div", { class: "menu-sep" }));
      }
      for (const e of groups.get(g).sort((a, b) => order(a) - order(b))) {
        el.append(
          h(
            "div",
            {
              class: "menu-item",
              role: "menuitem",
              onclick: () => {
                el.remove();
                commandsApi.executeCommand(e.command, ...args);
              },
            },
            commandTitle(e.command)
          )
        );
      }
    });
    document.body.append(el);
    const r = el.getBoundingClientRect();
    el.style.left = `${Math.min(x, innerWidth - r.width - 4)}px`;
    el.style.top = `${Math.min(y, innerHeight - r.height - 4)}px`;
    setTimeout(() => document.addEventListener("mousedown", function off(ev) {
      if (!el.contains(ev.target)) {
        el.remove();
        document.removeEventListener("mousedown", off);
      }
    }), 0);
  }

  // ------------------------------------------------------------ the tree view

  function createTreeView(id, options) {
    const provider = options.treeDataProvider;
    const root = $("tree");
    const expanded = new Map();
    const selectionChanged = new EventEmitter();
    let selected = null;
    let drawing = Promise.resolve();
    let pending = false;
    const label = (item) => (typeof item.label === "object" && item.label ? item.label.label : item.label) || "";

    async function children(el, parentKey, depth, into) {
      let kids;
      try {
        kids = (await provider.getChildren(el)) || [];
      } catch (e) {
        console.error(e);
        kids = [];
      }
      const rows = [];
      let i = 0;
      for (const k of kids) {
        const item = await provider.getTreeItem(k);
        const key = item.id || `${parentKey}/${label(item)}#${i++}`;
        rows.push({ k, item, key });
      }
      for (const { k, item, key } of rows) {
        const can = item.collapsibleState > 0;
        if (can && !expanded.has(key)) {
          expanded.set(key, item.collapsibleState === 2);
        }
        const open = can && expanded.get(key);
        const ctx = { view: id, viewItem: item.contextValue || "" };
        const inline = menu("view/item/context", ctx).filter((e) => e.group && e.group.startsWith("inline"));
        const icon = item.iconPath instanceof ThemeIcon ? item.iconPath : null;
        const tip = item.tooltip && typeof item.tooltip === "object" ? item.tooltip.value : item.tooltip;
        const row = h(
          "div",
          {
            class: "tree-row" + (selected === key ? " selected" : ""),
            role: "treeitem",
            "aria-expanded": can ? String(!!open) : undefined,
            tabindex: "0",
            title: tip ? String(tip).replace(/\\([\\`*_{}[\]()#+\-.!<>])/g, "$1").replace(/\*\*/g, "") : undefined,
            style: `padding-left:${8 + depth * 12}px`,
          },
          h("span", { class: "twistie codicon " + (can ? (open ? "codicon-chevron-down" : "codicon-chevron-right") : "") }),
          icon
            ? h("span", {
                class: `tree-icon codicon codicon-${icon.id}`,
                style: icon.color ? `color: var(--vscode-${icon.color.id.replace(/\./g, "-")})` : undefined,
              })
            : null,
          h("span", { class: "tree-label" }, label(item)),
          item.description ? h("span", { class: "tree-desc" }, String(item.description)) : null,
          h(
            "span",
            { class: "tree-actions" },
            inline.map((e) =>
              h("span", {
                class: `tree-action codicon codicon-${commandIcon(e.command) || "circle-small"}`,
                title: commandTitle(e.command),
                onclick: (ev) => {
                  ev.stopPropagation();
                  commandsApi.executeCommand(e.command, k);
                },
              })
            )
          )
        );
        const toggle = () => {
          expanded.set(key, !expanded.get(key));
          redraw();
        };
        row.addEventListener("click", () => {
          selected = key;
          root.querySelectorAll(".tree-row.selected").forEach((r) => r.classList.remove("selected"));
          row.classList.add("selected");
          selectionChanged.fire({ selection: [k] });
          if (item.command) {
            commandsApi.executeCommand(item.command.command, ...(item.command.arguments || []));
          } else if (can) {
            toggle();
          }
        });
        row.querySelector(".twistie").addEventListener("click", (ev) => {
          if (can) {
            ev.stopPropagation();
            toggle();
          }
        });
        row.addEventListener("keydown", (ev) => {
          if (ev.key === "Enter" || ev.key === " ") {
            ev.preventDefault();
            row.click();
          } else if ((ev.key === "ArrowRight" && can && !open) || (ev.key === "ArrowLeft" && can && open)) {
            toggle();
          }
        });
        row.addEventListener("contextmenu", (ev) => {
          ev.preventDefault();
          contextMenu(ev.clientX, ev.clientY, menu("view/item/context", ctx).filter((e) => !(e.group || "").startsWith("inline")), [k]);
        });
        into.append(row);
        if (open) {
          await children(k, key, depth + 1, into);
        }
      }
    }

    function redraw() {
      if (pending) {
        return drawing;
      }
      pending = true;
      drawing = drawing.then(async () => {
        pending = false;
        const into = document.createDocumentFragment();
        await children(undefined, "", 0, into);
        const scroll = root.scrollTop;
        root.textContent = "";
        root.append(into);
        root.scrollTop = scroll;
        $("sidebar-empty").hidden = root.childElementCount > 0;
      });
      return drawing;
    }

    if (provider.onDidChangeTreeData) {
      provider.onDidChangeTreeData(() => redraw());
    }
    // The title bar's buttons, and its … menu.
    const title = $("sidebar-actions");
    title.textContent = "";
    const entries = menu("view/title", { view: id });
    for (const e of entries.filter((e) => (e.group || "").startsWith("navigation"))) {
      title.append(h("span", { class: `title-action codicon codicon-${commandIcon(e.command) || "circle-small"}`, title: commandTitle(e.command), role: "button", tabindex: "0", onclick: () => commandsApi.executeCommand(e.command) }));
    }
    const more = entries.filter((e) => !(e.group || "").startsWith("navigation"));
    if (more.length) {
      const btn = h("span", { class: "title-action codicon codicon-ellipsis", title: "More Actions…", role: "button", tabindex: "0" });
      btn.addEventListener("click", (ev) => {
        const r = btn.getBoundingClientRect();
        contextMenu(r.left, r.bottom, more, []);
        ev.stopPropagation();
      });
      title.append(btn);
    }
    redraw();
    return {
      onDidChangeSelection: selectionChanged.event,
      onDidChangeVisibility: new EventEmitter().event,
      onDidCollapseElement: new EventEmitter().event,
      onDidExpandElement: new EventEmitter().event,
      visible: true,
      get selection() {
        return [];
      },
      async reveal(el, opts) {
        // (Its entry: expanded, as asked.)
        if (opts && opts.expand) {
          const item = await provider.getTreeItem(el);
          const kids = (await provider.getChildren(undefined)) || [];
          let i = 0;
          for (const k of kids) {
            const it = await provider.getTreeItem(k);
            const key = it.id || `/${label(it)}#${i++}`;
            if (label(it) === label(item)) {
              expanded.set(key, true);
            }
          }
          await redraw();
        }
      },
      dispose() {},
      message: undefined,
      title: undefined,
    };
  }

  // ------------------------------------------------------------ webviews

  /** The page's URL of an extension file (a webview's asWebviewUri). */
  let extensionBase = "ext/";
  function resourceUrl(uri) {
    const p = uri.path.replace(/^\/ext\//, "");
    return new URL(extensionBase + p, location.href).href;
  }

  /** VS Code's theme variables, for a webview (from the page's own). */
  function themeCss() {
    const style = getComputedStyle(document.documentElement);
    const names = [];
    for (const sheet of document.styleSheets) {
      let rules;
      try {
        rules = sheet.cssRules;
      } catch {
        continue;
      }
      for (const r of rules) {
        if (r.style) {
          for (const n of r.style) {
            if (n.startsWith("--vscode-")) {
              names.push(n);
            }
          }
        }
      }
    }
    const vars = [...new Set(names)].map((n) => `${n}: ${style.getPropertyValue(n)};`).join("\n");
    return `html { ${vars} }`;
  }

  /** What VS Code puts in every webview: its defaults, the API, the theme. */
  function webviewPrelude(viewType, state) {
    const dark = document.documentElement.classList.contains("dark");
    return `<style id="_defaultStyles">
${themeCss()}
html { scrollbar-color: var(--vscode-scrollbarSlider-background) transparent; }
body { background-color: transparent; color: var(--vscode-foreground); font-family: var(--vscode-font-family); font-weight: var(--vscode-font-weight); font-size: var(--vscode-font-size); margin: 0; padding: 0 20px; }
img, video { max-width: 100%; max-height: 100%; }
a, a code { color: var(--vscode-textLink-foreground); }
a:hover { color: var(--vscode-textLink-activeForeground); }
code { font-family: var(--vscode-editor-font-family); color: var(--vscode-textPreformat-foreground); }
:focus { outline: 1px solid -webkit-focus-ring-color; outline-offset: -1px; }
</style>
<script>
(function () {
  let state = ${JSON.stringify(state ?? null)};
  const api = {
    postMessage: (m) => parent.postMessage({ __hexcavatorWebview: true, m }, "*"),
    getState: () => state,
    setState: (s) => { state = s; parent.postMessage({ __hexcavatorWebview: true, state: s }, "*"); return s; },
  };
  let given = false;
  window.acquireVsCodeApi = () => { if (given) throw new Error("An instance of the VS Code API has already been acquired"); given = true; return api; };
  document.addEventListener("DOMContentLoaded", () => {
    document.body.classList.add("${dark ? "vscode-dark" : "vscode-light"}");
    document.body.dataset.vscodeThemeKind = "${dark ? "vscode-dark" : "vscode-light"}";
  });
  // Context menus: the clicked element's data-vscode-context (its
  // ancestors' merged under it), for the page's webview/context menu.
  document.addEventListener("contextmenu", (e) => {
    let ctx = {};
    const chain = [];
    for (let el = e.target; el && el.getAttribute; el = el.parentElement) {
      const c = el.getAttribute("data-vscode-context");
      if (c) chain.unshift(c);
    }
    for (const c of chain) { try { Object.assign(ctx, JSON.parse(c)); } catch {} }
    if (Object.keys(ctx).length) {
      e.preventDefault();
      parent.postMessage({ __hexcavatorWebview: true, contextmenu: { x: e.clientX, y: e.clientY, ctx } }, "*");
    }
  });
  // (Links out of the page: a new tab.)
  document.addEventListener("click", (e) => {
    const a = e.target.closest && e.target.closest("a[href]");
    if (a && /^https?:/.test(a.getAttribute("href"))) { e.preventDefault(); window.open(a.href, "_blank", "noopener"); }
  });
})();
</script>`;
  }

  function createWebviewPanel(viewType, title, _column, options) {
    const received = new EventEmitter();
    const disposed = new EventEmitter();
    const viewState = new EventEmitter();
    const frame = h("iframe", { class: "webview", title });
    const body = h("div", { class: "editor" }, frame);
    let html = "";
    let ready = false;
    let queue = [];
    let state;
    const webview = {
      options: options || {},
      cspSource: location.origin,
      asWebviewUri: (u) => Uri.parse(resourceUrl(u)),
      onDidReceiveMessage: received.event,
      postMessage(m) {
        if (ready && frame.contentWindow) {
          frame.contentWindow.postMessage(m, "*");
        } else {
          queue.push(m);
        }
        return Promise.resolve(true);
      },
      get html() {
        return html;
      },
      set html(v) {
        html = v;
        ready = false;
        // (No CSP: the page is its own; the prelude first, in <head>.)
        const page = String(v)
          .replace(/<meta[^>]+Content-Security-Policy[^>]*>/i, "")
          .replace(/<head>/i, `<head>${webviewPrelude(viewType, state)}`);
        frame.srcdoc = page;
      },
    };
    frame.addEventListener("load", () => {
      ready = true;
      const q = queue;
      queue = [];
      q.forEach((m) => frame.contentWindow.postMessage(m, "*"));
    });
    const onMessage = (e) => {
      if (e.source !== frame.contentWindow || !e.data || !e.data.__hexcavatorWebview) {
        return;
      }
      if ("m" in e.data) {
        received.fire(e.data.m);
      } else if ("state" in e.data) {
        state = e.data.state;
      } else if (e.data.contextmenu) {
        const { x, y, ctx } = e.data.contextmenu;
        const r = frame.getBoundingClientRect();
        const entries = menu("webview/context", Object.assign({ webviewId: viewType }, ctx));
        contextMenu(r.left + x, r.top + y, entries, [ctx]);
      }
    };
    window.addEventListener("message", onMessage);
    const tab = { title, body };
    const panel = {
      viewType,
      webview,
      title,
      iconPath: undefined,
      visible: true,
      active: true,
      options: options || {},
      onDidDispose: disposed.event,
      onDidChangeViewState: viewState.event,
      reveal() {
        activate(tab);
      },
      dispose() {
        closeTab(tab);
      },
    };
    Object.defineProperty(panel, "title", {
      get: () => tab.title,
      set: (t) => setTabTitle(tab, t),
    });
    tab.onClose = () => {
      window.removeEventListener("message", onMessage);
      disposed.fire();
    };
    tab.onViewState = (on) => {
      panel.active = on;
      panel.visible = on;
      viewState.fire({ webviewPanel: panel });
    };
    addTab(tab);
    return panel;
  }

  // ------------------------------------------------------------ text documents

  const providers = new Map();
  const documents = new Map();
  async function openTextDocument(uri) {
    if (!(uri instanceof Uri)) {
      uri = typeof uri === "string" ? Uri.file(uri) : Uri.from(uri);
    }
    const key = uri.toString();
    let text;
    const p = providers.get(uri.scheme);
    if (p) {
      text = await p.provideTextDocumentContent(uri, new CancellationTokenSource().token);
    } else {
      text = await window.HexcavatorNode.fs.promises.readFile(uri.fsPath, "utf8");
    }
    const lines = String(text ?? "").split(/\r?\n/);
    const doc = {
      uri,
      fileName: uri.fsPath,
      languageId: "plaintext",
      lineCount: lines.length,
      isUntitled: false,
      getText: () => String(text ?? ""),
      lineAt: (n) => ({ text: lines[typeof n === "number" ? n : n.line] || "" }),
    };
    documents.set(key, doc);
    return doc;
  }

  function textTab(title, text) {
    const pre = h("pre", { class: "text" }, text);
    const body = h("div", { class: "editor text-editor" }, pre);
    addTab({ title, body });
    return pre;
  }

  async function showTextDocument(doc, options) {
    const d = doc.uri ? doc : await openTextDocument(doc);
    const name = d.uri.path.split("/").pop() || d.uri.path;
    const pre = textTab(decodeURIComponent(name), d.getText());
    const line = options && options.selection && options.selection.start ? options.selection.start.line : null;
    if (line != null) {
      requestAnimationFrame(() => {
        const lh = parseFloat(getComputedStyle(pre).lineHeight) || 18;
        pre.parentElement.scrollTop = Math.max(0, line * lh - 80);
      });
    }
    return { document: d };
  }

  async function showDiff(left, right, title) {
    const [a, b] = await Promise.all([openTextDocument(left), openTextDocument(right)]);
    const body = h(
      "div",
      { class: "editor diff-editor" },
      h("pre", { class: "text" }, a.getText()),
      h("pre", { class: "text" }, b.getText())
    );
    addTab({ title: title || "Diff", body });
  }

  // ------------------------------------------------------------ output, settings

  let outputLines = [];
  function createOutputChannel(name) {
    let tab = null;
    let pre = null;
    return {
      name,
      appendLine(t) {
        outputLines.push(t);
        if (outputLines.length > 5000) {
          outputLines = outputLines.slice(-4000);
        }
        if (pre) {
          pre.append(t + "\n");
        }
      },
      append(t) {
        this.appendLine(t);
      },
      show() {
        if (tab && tab.body.isConnected) {
          activate(tab);
          return;
        }
        pre = h("pre", { class: "text" }, outputLines.join("\n") + "\n");
        tab = { title: `Output: ${name}`, body: h("div", { class: "editor text-editor" }, pre) };
        tab.onClose = () => {
          pre = null;
        };
        addTab(tab);
      },
      hide() {},
      clear() {
        outputLines = [];
        if (pre) {
          pre.textContent = "";
        }
      },
      dispose() {},
    };
  }

  function openSettings() {
    const props = ((manifest.contributes.configuration || {}).properties) || {};
    const config = getConfiguration();
    const rows = [];
    for (const [key, p] of Object.entries(props)) {
      const v = config.get(key);
      let input;
      if (p.type === "boolean") {
        input = h("input", { type: "checkbox", checked: v ? true : undefined });
        input.addEventListener("change", () => config.update(key, input.checked));
      } else if (Array.isArray(p.enum)) {
        input = h("select", {}, p.enum.map((x) => h("option", { value: x, selected: x === v ? true : undefined }, x)));
        input.addEventListener("change", () => config.update(key, input.value));
      } else if (p.type === "string") {
        input = h("input", { type: "text", value: v || "" });
        input.addEventListener("change", () => config.update(key, input.value));
      } else {
        input = h("textarea", { rows: "3" }, JSON.stringify(v ?? null, null, 1));
        input.addEventListener("change", () => {
          try {
            config.update(key, JSON.parse(input.value));
            input.classList.remove("bad");
          } catch {
            input.classList.add("bad");
          }
        });
      }
      rows.push(
        h(
          "div",
          { class: "setting" },
          h("div", { class: "setting-name" }, key.replace(/^hexcavator\./, "Hexcavator: ")),
          h("div", { class: "setting-desc" }, p.markdownDescription || p.description || ""),
          input
        )
      );
    }
    addTab({ title: "Settings", body: h("div", { class: "editor settings" }, h("h2", {}, "Hexcavator settings"), h("p", { class: "setting-desc" }, "Kept in this browser."), rows) });
  }

  // ------------------------------------------------------------ files: dialogs

  /** Set by the page: picks files and adds them, resolving with their paths. */
  let pickFiles = async () => [];
  /** Set by the page: saves bytes as a download. */
  let download = () => {};

  const windowApi = {
    showInformationMessage: (m, ...items) => notify("info", m, items.filter((x) => typeof x !== "object" || x.title)),
    showWarningMessage: (m, ...items) => notify("warning", m, items.filter((x) => typeof x !== "object" || x.title)),
    showErrorMessage: (m, ...items) => notify("error", m, items.filter((x) => typeof x !== "object" || x.title)),
    setStatusBarMessage(text, timeout) {
      status.message = text;
      status.render();
      const clear = () => {
        if (status.message === text) {
          status.message = null;
          status.render();
        }
      };
      if (typeof timeout === "number") {
        setTimeout(clear, timeout);
      }
      return new Disposable(clear);
    },
    createStatusBarItem(_alignment, priority) {
      const it = {
        text: "",
        tooltip: undefined,
        command: undefined,
        priority: priority || 0,
        visible: false,
        show() {
          this.visible = true;
          status.render();
        },
        hide() {
          this.visible = false;
          status.render();
        },
        dispose() {
          status.items = status.items.filter((x) => x !== it);
          status.render();
        },
      };
      status.items.push(it);
      return new Proxy(it, {
        set(t, k, v) {
          t[k] = v;
          if (t.visible) {
            status.render();
          }
          return true;
        },
      });
    },
    createOutputChannel,
    withProgress,
    createQuickPick,
    showQuickPick,
    showInputBox,
    async showOpenDialog(options) {
      const paths = await pickFiles(options || {});
      return paths.length ? paths.map((p) => Uri.file(p)) : undefined;
    },
    async showSaveDialog(options) {
      const name = options && options.defaultUri ? options.defaultUri.path.split("/").pop() : "hexcavator-output.txt";
      return Uri.file(`/downloads/${name}`);
    },
    createWebviewPanel,
    registerCustomEditorProvider: () => new Disposable(),
    createTreeView,
    registerTreeDataProvider: (id, provider) => createTreeView(id, { treeDataProvider: provider }),
    showTextDocument,
    createTerminal() {
      notify("info", "The web version has no terminal: copy the command line instead.", []);
      return { show() {}, dispose() {}, sendText() {} };
    },
    get activeTextEditor() {
      return undefined;
    },
    visibleTextEditors: [],
    onDidChangeActiveTextEditor: new EventEmitter().event,
  };

  const workspaceApi = {
    getConfiguration,
    onDidChangeConfiguration: configChanged.event,
    registerTextDocumentContentProvider(scheme, provider) {
      providers.set(scheme, provider);
      return new Disposable(() => providers.delete(scheme));
    },
    openTextDocument,
    get textDocuments() {
      return [...documents.values()];
    },
    workspaceFolders: undefined,
    createFileSystemWatcher() {
      const e = new EventEmitter();
      return { onDidChange: e.event, onDidCreate: e.event, onDidDelete: e.event, dispose() {} };
    },
    fs: {
      async writeFile(uri, bytes) {
        download(uri.path.split("/").pop(), bytes);
      },
      async readFile(uri) {
        return window.HexcavatorNode.fs.promises.readFile(uri.fsPath);
      },
      async stat(uri) {
        const s = window.HexcavatorNode.fs.statSync(uri.fsPath);
        return { type: s.isDirectory() ? 2 : 1, size: s.size, mtime: s.mtimeMs, ctime: s.mtimeMs };
      },
    },
    onDidOpenTextDocument: new EventEmitter().event,
    onDidCloseTextDocument: new EventEmitter().event,
    onDidChangeTextDocument: new EventEmitter().event,
  };

  const vscode = {
    version: "1.85.0-web",
    Disposable,
    EventEmitter,
    Uri,
    CancellationTokenSource,
    MarkdownString,
    TreeItem,
    TreeItemCollapsibleState: { None: 0, Collapsed: 1, Expanded: 2 },
    ThemeIcon,
    ThemeColor,
    Position,
    Range,
    Selection,
    CodeLens,
    RelativePattern,
    ViewColumn: { Active: -1, Beside: -2, One: 1, Two: 2, Three: 3 },
    StatusBarAlignment: { Left: 1, Right: 2 },
    ProgressLocation: { SourceControl: 1, Window: 10, Notification: 15 },
    FileType: { Unknown: 0, File: 1, Directory: 2, SymbolicLink: 64 },
    commands: commandsApi,
    window: windowApi,
    workspace: workspaceApi,
    languages: {
      setTextDocumentLanguage: async (doc, id) => Object.assign(doc, { languageId: id }),
      registerCodeLensProvider: () => new Disposable(),
    },
    env: {
      clipboard: {
        writeText: (t) => navigator.clipboard.writeText(t).catch(() => {}),
        readText: () => navigator.clipboard.readText().catch(() => ""),
      },
      openExternal: async (u) => window.open(String(u), "_blank", "noopener"),
      appName: "Hexcavator for the web",
      uiKind: 2,
    },
    extensions: { getExtension: () => undefined },
  };

  window.HexcavatorVscode = {
    api: vscode,
    /** The extension's package.json, for menus, commands, and settings. */
    setManifest(m) {
      manifest = m;
    },
    setPickFiles(f) {
      pickFiles = f;
    },
    setDownload(f) {
      download = f;
    },
    setExtensionBase(b) {
      extensionBase = b;
    },
    runLink,
    memento,
    when,
    icons,
  };
})();
