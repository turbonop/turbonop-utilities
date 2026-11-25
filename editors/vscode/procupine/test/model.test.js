// Tests for media/model.js: `node test/model.test.js`.
"use strict";

const assert = require("assert");
const path = require("path");
const m = require(path.join(__dirname, "..", "media", "model.js"));

const COLUMNS = ["id", "parent", "cpu", "private", "resident", "virtual", "threads", "handles", "readRate", "writeRate", "ioOps", "contextSwitches", "pageFaults", "priority", "state", "userTime", "kernelTime", "readBytes", "writeBytes"];

function proc(id, pid, parent, name, extra = {}) {
  return { id, pid, parent, name, start: 1000 + id, user: "me", path: `/bin/${name}`, commandLine: `${name} --flag`, ...extra };
}

function row(id, parent, cpu, priv, extra = {}) {
  const r = COLUMNS.map(() => 0);
  r[0] = id;
  r[1] = parent;
  r[2] = cpu;
  r[3] = priv;
  r[8] = extra.readRate || 0;
  r[9] = extra.writeRate || 0;
  r[14] = extra.state || "sleeping";
  r[15] = extra.userTime || 0;
  r[16] = extra.kernelTime || 0;
  return r;
}

const system = { cpus: 4, cpu: 0.5, memoryTotal: 100, memoryAvailable: 25, commit: 10, commitLimit: 40 };

function sample() {
  const model = new m.Model(COLUMNS);
  model.apply({
    added: [proc(1, 1, null, "init"), proc(2, 20, 1, "shell"), proc(3, 30, 2, "editor"), proc(4, 40, 1, "daemon"), proc(5, 50, null, "kthreadd", { kernel: true })],
    removed: [],
    rows: [row(1, null, 0.01, 100), row(2, 1, 0.2, 300), row(3, 2, 0.5, 900, { readRate: 10, writeRate: 5 }), row(4, 1, 0, 50), row(5, null, 0, 0)],
    system,
  });
  return model;
}

const names = (rows) => rows.map((r) => r.p.name);
const depths = (rows) => rows.map((r) => [r.p.name, r.depth]);

// The tree: by parent, siblings by start.
{
  const model = sample();
  assert.deepStrictEqual(depths(model.rows()), [["init", 0], ["shell", 1], ["editor", 2], ["daemon", 1], ["kthreadd", 0]]);
  const rows = model.rows();
  assert.strictEqual(rows[0].kids, true);
  assert.strictEqual(rows[2].kids, false);
  // Collapsed: its descendants hidden.
  assert.deepStrictEqual(names(model.rows({ collapsed: new Set([2]) })), ["init", "shell", "daemon", "kthreadd"]);
  // Siblings sorted by a column.
  assert.deepStrictEqual(names(model.rows({ sort: { key: "private", desc: true } })), ["init", "shell", "editor", "daemon", "kthreadd"]);
  assert.deepStrictEqual(names(model.rows({ sort: { key: "private", desc: false } })), ["kthreadd", "init", "daemon", "shell", "editor"]);
  // Flat, by CPU.
  assert.deepStrictEqual(names(model.rows({ tree: false, sort: { key: "cpu", desc: true } })), ["editor", "shell", "init", "daemon", "kthreadd"]);
  // Hidden (kernel threads).
  assert.deepStrictEqual(names(model.rows({ hide: (p) => p.kernel })), ["init", "shell", "editor", "daemon"]);
}

// Filtered: matches, with their ancestors (not matching) in a tree.
{
  const model = sample();
  const rows = model.rows({ filter: "EDIT" });
  assert.deepStrictEqual(names(rows), ["init", "shell", "editor"]);
  assert.deepStrictEqual(rows.map((r) => r.match), [false, false, true]);
  // Collapsed ancestors open to show a match.
  assert.deepStrictEqual(names(model.rows({ filter: "editor", collapsed: new Set([1, 2]) })), ["init", "shell", "editor"]);
  assert.deepStrictEqual(names(model.rows({ tree: false, filter: "40" })), ["daemon"]);
  assert.deepStrictEqual(names(model.rows({ tree: false, filter: "--flag" })).length, 5);
  assert.deepStrictEqual(names(model.rows({ tree: false, filter: "/bin/sh" })), ["shell"]);
  assert.deepStrictEqual(names(model.rows({ filter: "nothing" })), []);
}

// Samples after the first: rows that changed, processes started and exited.
{
  const model = sample();
  assert.strictEqual(model.isNew(model.get(1)), false, "the first sample's processes aren't new");
  const updated = model.apply({ added: [proc(6, 60, 3, "child")], removed: [4], rows: [row(3, 2, 0.25, 950), row(6, 3, 0.1, 10)], system: { ...system, cpu: 0.75 } });
  assert.strictEqual(updated.size, 2);
  assert.strictEqual(model.get(3).v.cpu, 0.25);
  // (Unsent rows are unchanged.)
  assert.strictEqual(model.get(2).v.cpu, 0.2);
  assert.strictEqual(model.isNew(model.get(6)), true);
  assert.strictEqual(model.isExited(model.get(4)), true);
  assert.deepStrictEqual(names(model.rows()), ["init", "shell", "editor", "child", "daemon", "kthreadd"]);
  // History: every live process's, sent or not.
  assert.deepStrictEqual(model.get(3).history.cpu.list(), [0.5, 0.25]);
  assert.deepStrictEqual(model.get(2).history.cpu.list(), [0.2, 0.2]);
  assert.deepStrictEqual(model.get(6).history.cpu.list(), [0.1]);
  assert.deepStrictEqual(model.get(3).history.io.list(), [15, 0]);
  assert.deepStrictEqual(model.systemHistory.cpu.list(), [0.5, 0.75]);
  assert.deepStrictEqual(model.systemHistory.memory.list(), [0.75, 0.75]);
  // The next sample: the exited gone, the new not new.
  model.apply({ added: [], removed: [], rows: [] });
  assert.strictEqual(model.get(4), undefined);
  assert.strictEqual(model.isNew(model.get(6)), false);
  assert.deepStrictEqual(model.descendants(1), [1, 2, 3, 6]);
  assert.deepStrictEqual(model.ancestors(6), [3, 2, 1]);
  // A refresh: everything again, history kept.
  model.apply({ added: [proc(3, 30, 2, "editor")], removed: [], rows: [row(3, 2, 0.3, 950)] });
  assert.deepStrictEqual(model.get(3).history.cpu.list(), [0.5, 0.25, 0.25, 0.3]);
}

// A process whose parent isn't known is a root; a loop doesn't hang.
{
  const model = new m.Model(COLUMNS);
  model.apply({ added: [proc(1, 1, 99, "orphan"), proc(2, 2, 3, "a"), proc(3, 3, 2, "b")], rows: [row(1, 99, 0, 0), row(2, 3, 0, 0), row(3, 2, 0, 0)] });
  assert.deepStrictEqual(names(model.rows()), ["orphan"]);
  assert.deepStrictEqual(model.descendants(2), [2, 3]);
  assert.deepStrictEqual(model.ancestors(2), [3]);
}

// The ring.
{
  const r = new m.Ring(3);
  assert.deepStrictEqual(r.list(), []);
  assert.strictEqual(r.last(), 0);
  [1, 2, 3, 4].forEach((v) => r.push(v));
  assert.deepStrictEqual(r.list(), [2, 3, 4]);
  assert.strictEqual(r.last(), 4);
  assert.strictEqual(r.max(), 4);
  r.push(NaN);
  assert.deepStrictEqual(r.list(), [3, 4, 0]);
}

// Formatting.
assert.strictEqual(m.bytes(0), "0 B");
assert.strictEqual(m.bytes(1023), "1023 B");
assert.strictEqual(m.bytes(1024), "1.00 KiB");
assert.strictEqual(m.bytes(150 * 1024 * 1024), "150 MiB");
assert.strictEqual(m.bytes(15.5 * 1024 * 1024), "15.5 MiB");
assert.strictEqual(m.bytes(null), "");
assert.strictEqual(m.rate(0), "");
assert.strictEqual(m.rate(2048), "2.00 KiB/s");
assert.strictEqual(m.percent(0), "");
assert.strictEqual(m.percent(0.00001), "< 0.01");
assert.strictEqual(m.percent(0.1234), "12.34");
assert.strictEqual(m.duration(3723004), "1:02:03.004");
assert.strictEqual(m.duration(null), "");
assert.strictEqual(m.count(12345), "12,345");
assert.strictEqual(m.count(null), "");
const noon = new Date(2026, 10, 24, 12, 0, 0).getTime();
assert.strictEqual(m.startTime(noon - 3600000, noon), "11:00:00");
assert.strictEqual(m.startTime(new Date(2026, 10, 20, 9, 5, 7).getTime(), noon), "2026-11-20 09:05:07");
assert.strictEqual(m.startTime(null), "");
assert.strictEqual(m.sparkPoints([], 10, 10, 1), "");
assert.strictEqual(m.sparkPoints([0, 1], 10, 10, 1, 2), "0.0,9.5 10.0,0.5");
assert.strictEqual(m.isUser("DOMAIN\\Me", "me"), true);
assert.strictEqual(m.isUser("me", "me"), true);
assert.strictEqual(m.isUser("root", "me"), false);
assert.strictEqual(m.isUser(null, "me"), false);
assert.strictEqual(m.sortValue({ v: {}, pid: 1, history: { cpu: new m.Ring() } }, "private"), -Infinity);

console.log("model: ok");
