// Dump Analyzer: crash dumps in VS Code's debugger. An inline debug adapter
// (lib/session.js) answers the debugger's requests from `dump-analyzer
// serve`, one long-lived process that keeps each dump it opens.
"use strict";

const vscode = require("vscode");
const path = require("path");
const fs = require("fs");
const { spawn } = require("child_process");
const { DumpSession } = require("./lib/session");

const DUMPS = { "Crash dumps": ["dmp", "mdmp", "core"], "All files": ["*"] };

/** `dump-analyzer serve`: requests as JSON lines, answered as they finish. */
class Server {
  constructor(exe, args) {
    this.exe = exe;
    this.args = args;
    this.proc = null;
    this.pending = new Map();
    this.next = 1;
    this.buffer = "";
  }

  start() {
    if (this.proc) {
      return;
    }
    this.proc = spawn(this.exe, this.args, { stdio: ["pipe", "pipe", "pipe"] });
    this.proc.stdout.setEncoding("utf8");
    this.proc.stdout.on("data", (chunk) => {
      this.buffer += chunk;
      let nl;
      while ((nl = this.buffer.indexOf("\n")) >= 0) {
        const line = this.buffer.slice(0, nl);
        this.buffer = this.buffer.slice(nl + 1);
        if (!line.trim()) {
          continue;
        }
        let answer;
        try {
          answer = JSON.parse(line);
        } catch {
          continue;
        }
        const p = this.pending.get(answer.id);
        if (p) {
          this.pending.delete(answer.id);
          if (answer.error) {
            p.reject(new Error(answer.error));
          } else {
            p.resolve(answer.json);
          }
        }
      }
    });
    const gone = (e) => {
      for (const p of this.pending.values()) {
        p.reject(e);
      }
      this.pending.clear();
      this.proc = null;
    };
    this.proc.on("error", (e) =>
      gone(new Error(e.code === "ENOENT" ? `dump-analyzer was not found ("${this.exe}"); set dumpAnalyzer.path` : e.message)),
    );
    this.proc.on("exit", () => gone(new Error("dump-analyzer exited")));
  }

  request(args) {
    this.start();
    const id = this.next++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.proc.stdin.write(JSON.stringify({ id, args }) + "\n");
    });
  }

  dispose() {
    if (this.proc) {
      this.proc.kill();
      this.proc = null;
    }
  }
}

/** The program to run: the setting's, the one shipped in bin/, or the
 * PATH's. */
function program(context) {
  const configured = String(vscode.workspace.getConfiguration("dumpAnalyzer").get("path") || "").trim();
  if (configured) {
    return configured;
  }
  const shipped = path.join(context.extensionPath, "bin", process.platform === "win32" ? "dump-analyzer.exe" : "dump-analyzer");
  return fs.existsSync(shipped) ? shipped : "dump-analyzer";
}

function serverArgs() {
  const c = vscode.workspace.getConfiguration("dumpAnalyzer");
  const args = [];
  for (const url of [].concat(c.get("symbolServers") || [])) {
    if (String(url).trim()) {
      args.push("-symbol-server", String(url).trim());
    }
  }
  for (const url of [].concat(c.get("debuginfod") || [])) {
    if (String(url).trim()) {
      args.push("-debuginfod", String(url).trim());
    }
  }
  const cache = String(c.get("symbolCache") || "").trim();
  if (cache) {
    args.push("-symbol-cache", cache);
  }
  args.push("serve");
  return args;
}

/** An inline debug adapter over a session. */
class Adapter {
  constructor(server) {
    this.emitter = new vscode.EventEmitter();
    this.onDidSendMessage = this.emitter.event;
    this.session = new DumpSession(
      (args) => server.request(args),
      (m) => this.emitter.fire(m),
    );
  }
  handleMessage(message) {
    if (message.type === "request") {
      this.session.handle(message);
    }
  }
  dispose() {
    this.emitter.dispose();
  }
}

function activate(context) {
  let server = null;
  const ensure = () => {
    if (!server) {
      server = new Server(program(context), serverArgs());
    }
    return server;
  };
  context.subscriptions.push(
    { dispose: () => server && server.dispose() },
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("dumpAnalyzer") && server) {
        server.dispose();
        server = null;
      }
    }),
    vscode.debug.registerDebugAdapterDescriptorFactory("dump-analyzer", {
      createDebugAdapterDescriptor: () => new vscode.DebugAdapterInlineImplementation(new Adapter(ensure())),
    }),
    vscode.debug.registerDebugConfigurationProvider("dump-analyzer", {
      async resolveDebugConfiguration(_folder, config) {
        if (!config.dump) {
          const picked = await vscode.window.showOpenDialog({ title: "Crash dump to open", canSelectMany: false, filters: DUMPS });
          if (!picked || !picked.length) {
            return undefined;
          }
          config.dump = picked[0].fsPath;
        }
        config.type = "dump-analyzer";
        config.request = "launch";
        config.name = config.name || path.basename(config.dump);
        return config;
      },
    }),
    vscode.commands.registerCommand("dumpAnalyzer.openDump", async (uri) => {
      let dump = uri && uri.fsPath;
      if (!dump) {
        const picked = await vscode.window.showOpenDialog({ title: "Crash dump to open", canSelectMany: false, filters: DUMPS });
        if (!picked || !picked.length) {
          return;
        }
        dump = picked[0].fsPath;
      }
      await vscode.debug.startDebugging(undefined, {
        type: "dump-analyzer",
        request: "launch",
        name: path.basename(dump),
        dump,
      });
    }),
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
