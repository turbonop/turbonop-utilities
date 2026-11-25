// The debug adapter session (lib/session.js): with a server's canned
// answers, and (if it's built) with `dump-analyzer serve` itself on the
// repository's Windows dump. Run with `node --test test/`.
"use strict";

const test = require("node:test");
const assert = require("node:assert");
const path = require("path");
const fs = require("fs");
const { spawn } = require("child_process");
const { DumpSession, Handles, frameName, valueText } = require("../lib/session");

/** A session whose server answers from `answers` (by command), and the
 * messages it sent. */
function session(answers) {
  const sent = [];
  const asked = [];
  const s = new DumpSession(
    async (args) => {
      asked.push(args);
      const a = answers[args[0]];
      return typeof a === "function" ? a(args) : a;
    },
    (m) => sent.push(m),
  );
  return { s, sent, asked };
}

async function call(s, command, args) {
  const before = s.seq;
  await s.handle({ type: "request", seq: before, command, arguments: args });
}

const STACK = {
  thread: 7,
  end: "outermost",
  frames: [
    { index: 0, pc: "0x1010", sp: "0x500", module: "app.exe", function: "leaf", offset: "0x10", file: "c:\\src\\a.c", line: 30, confidence: "certain", inlines: [{ function: "helper", file: "c:\\src\\h.h", line: 5 }] },
    { index: 1, pc: "0x2020", sp: "0x540", module: "app.exe", function: "main", offset: "0x20", confidence: "certain", inlines: [] },
    { index: 2, pc: "0x9999", sp: "0x580", module: "kernel32.dll", confidence: "likely", inlines: [] },
  ],
};

test("handles", () => {
  const h = new Handles();
  const a = h.get({ x: 1 });
  assert.strictEqual(h.get({ x: 1 }), a);
  assert.notStrictEqual(h.get({ x: 2 }), a);
  assert.deepStrictEqual(h.lookup(a), { x: 1 });
  assert.strictEqual(h.lookup(999), undefined);
});

test("names and values", () => {
  assert.strictEqual(frameName(STACK.frames[0]), "app.exe!leaf+0x10");
  assert.strictEqual(frameName(STACK.frames[2]), "kernel32.dll!0x9999 (likely)");
  assert.strictEqual(valueText({ text: "42" }), "42");
  assert.strictEqual(valueText({ text: "", pointer: "0x10" }), "0x10");
  assert.strictEqual(valueText({ text: "", type: "node", children: [{}] }), "{node}");
  assert.strictEqual(valueText(null), "");
});

test("launch, stop at the exception, threads, stack", async () => {
  const { s, sent } = session({
    summary: { os: "Windows", arch: "x64", threads: 2, modules: 3, images_found: 3, debug_found: 1, exception: { thread: 7, name: "access violation", address: "0x1010" } },
    threads: [{ id: 7, name: "worker", frame: STACK.frames[0] }, { id: 8, frame: null }],
    stack: STACK,
  });
  await call(s, "initialize", {});
  assert.ok(sent[0].body.supportsDisassembleRequest);
  await call(s, "launch", { dump: "crash.dmp", symbols: ["c:\\syms"] });
  assert.strictEqual(sent.at(-1).event, "initialized");
  await call(s, "configurationDone", {});
  const stopped = sent.find((m) => m.event === "stopped");
  assert.strictEqual(stopped.body.threadId, 7);
  assert.strictEqual(stopped.body.reason, "exception");
  assert.match(stopped.body.text, /access violation/);
  await call(s, "threads", {});
  const threads = sent.at(-1).body.threads;
  assert.strictEqual(threads[0].name, "7 (worker) app.exe!leaf+0x10");
  await call(s, "stackTrace", { threadId: 7 });
  const frames = sent.at(-1).body.stackFrames;
  // The inlined call, then its frame, then the others.
  assert.strictEqual(frames.length, 4);
  assert.strictEqual(frames[0].name, "app.exe!helper (inlined)");
  assert.strictEqual(frames[0].line, 5);
  assert.strictEqual(frames[1].source.path, "c:\\src\\a.c");
  assert.strictEqual(frames[1].instructionPointerReference, "0x1010");
  assert.strictEqual(frames[3].presentationHint, "subtle");
  assert.strictEqual(frames[3].source, undefined);
  // The options reach every command.
  await call(s, "stackTrace", { threadId: 7, startFrame: 1, levels: 1 });
  assert.strictEqual(sent.at(-1).body.stackFrames.length, 1);
  assert.strictEqual(sent.at(-1).body.totalFrames, 4);
});

test("scopes, locals and their members, registers", async () => {
  const locals = [
    { name: "n", type: "node *", value: { type: "node *", text: "", pointer: "0x700", children: [] }, provenance: null },
    { name: "node", type: "node", value: { type: "node", text: "", children: [{ name: "next", type: "node *", text: "nullptr", children: [] }, { name: "value", type: "int", text: "42", children: [] }] }, provenance: "dump" },
    { name: "gone", type: "int", value: null, unavailable: "not live here (optimized out)" },
  ];
  const { s, sent, asked } = session({
    stack: STACK,
    locals,
    registers: [{ register: "rax", clobbered: true }, { register: "rsp", value: "0x540", from: "CFA" }],
  });
  await call(s, "launch", { dump: "crash.dmp", symbols: ["c:\\syms"] });
  await call(s, "stackTrace", { threadId: 7 });
  const frame = sent.at(-1).body.stackFrames[2];
  await call(s, "scopes", { frameId: frame.id });
  const [scopeLocals, scopeRegs] = sent.at(-1).body.scopes;
  await call(s, "variables", { variablesReference: scopeLocals.variablesReference });
  const vars = sent.at(-1).body.variables;
  assert.deepStrictEqual(asked.at(-1), ["locals", "crash.dmp", "-thread", "7", "-frame", "1", "-symbols", "c:\\syms"]);
  assert.strictEqual(vars[0].value, "0x700");
  assert.strictEqual(vars[0].memoryReference, "0x700");
  assert.strictEqual(vars[0].variablesReference, 0);
  assert.strictEqual(vars[1].value, "{node}");
  assert.ok(vars[1].variablesReference > 0);
  assert.strictEqual(vars[2].value, "<not live here (optimized out)>");
  await call(s, "variables", { variablesReference: vars[1].variablesReference });
  const members = sent.at(-1).body.variables;
  assert.deepStrictEqual(members.map((m) => [m.name, m.value]), [["next", "nullptr"], ["value", "42"]]);
  await call(s, "variables", { variablesReference: scopeRegs.variablesReference });
  const regs = sent.at(-1).body.variables;
  assert.strictEqual(regs[0].value, "<clobbered>");
  assert.strictEqual(regs[1].type, "from CFA");
});

test("memory and disassembly", async () => {
  const { s, sent, asked } = session({
    memory: [{ address: "0x1000", length: 4, provenance: "dump", bytes: "01020304" }, { address: "0x1008", length: 2, provenance: "image", bytes: "0506" }],
    disasm: [
      { address: "0x1000", bytes: "90", text: "nop" },
      { address: "0x1001", bytes: "90", text: "nop" },
      { address: "0x1002", bytes: "c3", text: "ret" },
    ],
  });
  await call(s, "launch", { dump: "crash.dmp" });
  await call(s, "readMemory", { memoryReference: "0x1000", count: 16 });
  const m = sent.at(-1).body;
  assert.strictEqual(Buffer.from(m.data, "base64").toString("hex"), "01020304");
  assert.strictEqual(m.unreadableBytes, 12);
  await call(s, "disassemble", { memoryReference: "0x1001", instructionOffset: -1, instructionCount: 3 });
  const d = sent.at(-1).body.instructions;
  assert.deepStrictEqual(d.map((i) => i.instruction), ["nop", "nop", "ret"]);
  assert.strictEqual(asked.at(-1)[2], "-address");
  await call(s, "disassemble", { memoryReference: "0x1000", instructionCount: 5 });
  const padded = sent.at(-1).body.instructions;
  assert.strictEqual(padded.length, 5);
  assert.strictEqual(padded[4].presentationHint, "invalid");
});

test("what a dump can't do, and errors", async () => {
  const { s, sent } = session({ boom: () => Promise.reject(new Error("no such thread")) });
  await call(s, "launch", { dump: "crash.dmp" });
  await call(s, "continue", {});
  assert.strictEqual(sent.at(-1).success, false);
  await call(s, "next", {});
  assert.match(sent.at(-1).message, /isn't supported/);
  await call(s, "setBreakpoints", { breakpoints: [{ line: 3 }] });
  assert.strictEqual(sent.at(-1).body.breakpoints[0].verified, false);
  await call(s, "evaluate", { expression: "boom 1" });
  assert.strictEqual(sent.at(-1).success, false);
  assert.match(sent.at(-1).message, /no such thread/);
  const fresh = session({});
  await call(fresh.s, "launch", {});
  assert.strictEqual(fresh.sent.at(-1).success, false);
});

/** `dump-analyzer serve`, if it's been built. */
function built() {
  const exe = process.platform === "win32" ? "dump-analyzer.exe" : "dump-analyzer";
  for (const profile of ["debug", "release"]) {
    const p = path.join(__dirname, "..", "..", "..", "..", "target", profile, exe);
    if (fs.existsSync(p)) {
      return p;
    }
  }
  return null;
}

test("the real server, on the repository's Windows dump", { skip: !built() && "dump-analyzer isn't built" }, async () => {
  const proc = spawn(built(), ["serve"], { stdio: ["pipe", "pipe", "inherit"] });
  const pending = new Map();
  let buffer = "";
  proc.stdout.setEncoding("utf8");
  proc.stdout.on("data", (c) => {
    buffer += c;
    let nl;
    while ((nl = buffer.indexOf("\n")) >= 0) {
      const a = JSON.parse(buffer.slice(0, nl));
      buffer = buffer.slice(nl + 1);
      const p = pending.get(a.id);
      pending.delete(a.id);
      if (a.error) {
        p.reject(new Error(a.error));
      } else {
        p.resolve(a.json);
      }
    }
  });
  let next = 1;
  const request = (args) =>
    new Promise((resolve, reject) => {
      const id = next++;
      pending.set(id, { resolve, reject });
      proc.stdin.write(JSON.stringify({ id, args }) + "\n");
    });
  const sent = [];
  const s = new DumpSession(request, (m) => sent.push(m));
  const dump = path.join(__dirname, "..", "..", "..", "..", "testdata", "minidump", "crash-x64.dmp");
  try {
    await call(s, "launch", { dump });
    await call(s, "configurationDone", {});
    const stopped = sent.find((m) => m.event === "stopped");
    assert.strictEqual(stopped.body.reason, "exception");
    await call(s, "stackTrace", { threadId: stopped.body.threadId });
    const frames = sent.at(-1).body.stackFrames;
    assert.match(frames[0].name, /crash-x64\.exe!leaf\+0x13/);
    assert.match(frames[1].name, /middle\+0x33/);
    await call(s, "scopes", { frameId: frames[1].id });
    await call(s, "variables", { variablesReference: sent.at(-1).body.scopes[0].variablesReference });
    const n = sent.at(-1).body.variables.find((v) => v.name === "n");
    assert.strictEqual(n.value, "0x848757f770");
  } finally {
    proc.stdin.end();
  }
});
