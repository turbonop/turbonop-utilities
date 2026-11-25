// Tests for lib/server.js, with a real procupine: `node test/server.test.js <procupine>`.
"use strict";

const assert = require("assert");
const path = require("path");
const { spawn } = require("child_process");
const { Server, procupinePath } = require(path.join(__dirname, "..", "lib", "server.js"));

// Where procupine is looked for.
{
  const none = () => false;
  assert.strictEqual(procupinePath("/x/procupine", none), "/x/procupine");
  assert.strictEqual(procupinePath("", none, "linux"), "procupine");
  const bundled = path.join(__dirname, "..", "bin", "procupine.exe");
  assert.strictEqual(procupinePath("", (p) => p === bundled, "win32"), bundled);
  const built = path.join(__dirname, "..", "..", "..", "..", "target", "debug", "procupine");
  assert.strictEqual(procupinePath("", (p) => p === built, "linux"), built);
}

const exe = process.argv[2];
if (!exe) {
  console.log("server: ok (without procupine: give its path to run it)");
  process.exit(0);
}

function timeout(ms, what) {
  return new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out: ${what}`)), ms));
}

async function main() {
  const events = [];
  let waiter = null;
  const server = new Server(exe, {
    onEvent: (e) => {
      events.push(e);
      if (waiter && waiter.test(e)) {
        waiter.resolve(e);
        waiter = null;
      }
    },
  });
  const next = (test) =>
    Promise.race([
      new Promise((resolve) => {
        const found = events.find(test);
        if (found) {
          events.splice(events.indexOf(found), 1);
          resolve(found);
        } else {
          waiter = { test, resolve };
        }
      }),
      timeout(20000, "an event"),
    ]);
  const columns = await server.request("columns");
  assert.strictEqual(columns[0], "id");
  assert.deepStrictEqual(await server.request("watch", { interval: 100 }), { interval: 100 });
  const first = await next((e) => e.event === "sample");
  const me = first.added.find((p) => p.pid === process.pid);
  assert.ok(me, "this process in the first sample");
  assert.ok(me.name.toLowerCase().startsWith("node"), me.name);
  // A child, killed by its ID.
  const child = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000)"]);
  const started = await next((e) => e.event === "sample" && e.added.some((p) => p.pid === child.pid));
  const id = started.added.find((p) => p.pid === child.pid).id;
  const exited = new Promise((resolve) => child.on("exit", resolve));
  const results = await server.request("kill", { processes: [id] });
  assert.deepStrictEqual(results, [{ process: id, ok: true }]);
  await Promise.race([exited, timeout(10000, "the child's exit")]);
  await next((e) => e.event === "sample" && e.removed.includes(id));
  await assert.rejects(server.request("nope"), /nope/);
  const ended = new Promise((resolve) => (server.onExit = resolve));
  server.child.stdin.end();
  const why = await Promise.race([ended, timeout(10000, "the server's exit")]);
  assert.match(why.message, /exited \(0\)/);
  console.log("server: ok");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
