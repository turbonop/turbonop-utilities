// Time by loop, above each loop's first source line: a CodeLens for each
// loop the engine finds in the sampled functions of an open source file
// (one of the trace's, matched as line heat matches it).
"use strict";

const { bestMatch } = require("./line-heat");

/** A share (0 to 1) as text. */
function percent(share) {
  return (100 * share).toFixed(share < 0.001 ? 3 : 1) + "%";
}

/** A function's name without its module (`module!name`). */
function shortName(label) {
  const bang = label.indexOf("!");
  return bang < 0 ? label : label.slice(bang + 1);
}

/**
 * What a loop's lens says: its share of the trace's samples (in it or in
 * what it calls), how many of them, its own share (if any), and its
 * function; and how many loops start at its line, if more than one (the
 * one shown is the one with the most time).
 */
function loopText(loop, total) {
  const n = (v) => v.toLocaleString("en-US");
  const share = total ? loop.samples / total : 0;
  const own = total ? loop.own / total : 0;
  const several = loop.loops > 1 ? `; ${n(loop.loops)} loops start here` : "";
  return (
    `↺ Loop: ${percent(share)} of the samples (${n(loop.samples)})` +
    `${own > 0 ? `, ${percent(own)} in its own instructions` : ""}, in ${shortName(loop.function)}${several}`
  );
}

/**
 * The loops of a source file, as CodeLenses: from the trace line heat
 * shows, each loop of the file's most sampled functions whose code is
 * here, above its first line.
 */
class LoopLens {
  /** Lenses through `vscode`'s API, for `heat`'s trace in `engine`'s session. */
  constructor(vscode, engine, heat, log) {
    this.vscode = vscode;
    this.engine = engine;
    this.heat = heat;
    this.log = log || (() => {});
    this.changed = new vscode.EventEmitter();
    this.onDidChangeCodeLenses = this.changed.event;
    this.files = null; // {trace, paths}: the trace's source files
    this.cache = new Map(); // "trace|path" → loops
  }

  /** The trace or its symbols changed: ask again. */
  refresh() {
    this.files = null;
    this.cache.clear();
    this.changed.fire();
  }

  enabled() {
    return this.vscode.workspace.getConfiguration("stackrabbit").get("loopLens", true);
  }

  async provideCodeLenses(document) {
    const trace = this.heat.trace;
    const session = this.engine.session;
    if (!trace || !session || !this.enabled() || document.uri.scheme !== "file") {
      return [];
    }
    try {
      if (!this.files || this.files.trace !== trace.id) {
        const r = await session.request("lines", { trace: trace.id, limit: 100000 });
        this.files = { trace: trace.id, paths: r.files.map((f) => f.path) };
      }
      const file = bestMatch(document.uri.fsPath, this.files.paths);
      if (!file) {
        return [];
      }
      const key = `${trace.id}|${file}`;
      let r = this.cache.get(key);
      if (!r) {
        r = await session.request("loops", { trace: trace.id, file });
        this.cache.set(key, r);
      }
      const lenses = [];
      for (const loop of r.loops) {
        const line = (loop.line || 0) - 1; // (The symbols count from 1.)
        if (line < 0 || line >= document.lineCount || loop.samples === 0) {
          continue;
        }
        const range = document.lineAt(line).range;
        lenses.push(new this.vscode.CodeLens(range, { title: loopText(loop, r.total), command: "" }));
      }
      return lenses;
    } catch (e) {
      this.log(`Loop times: ${e.message}`);
      return [];
    }
  }

  dispose() {
    this.changed.dispose();
  }
}

module.exports = { loopText, shortName, LoopLens };
