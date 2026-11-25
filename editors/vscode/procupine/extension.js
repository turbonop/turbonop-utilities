// Procupine: the processes running on this machine (or, in a remote
// window, the remote one), as a process explorer shows them. One panel
// (media/view.js) shows what `procupine serve` samples (lib/server.js).
"use strict";

const vscode = require("vscode");
const os = require("os");
const crypto = require("crypto");
const { Server, procupinePath } = require("./lib/server.js");

let channel = null;
/** The panel, while it is open. */
let panel = null;
let server = null;
/** The server's columns, once asked. */
let columns = null;

function log(text) {
  if (channel) {
    channel.appendLine(text);
  }
}

function config() {
  return vscode.workspace.getConfiguration("procupine");
}

function hexcavatorInstalled() {
  return !!vscode.extensions.getExtension("turbonop.hexcavator-vscode");
}

/** The server, started if it isn't running. */
function ensureServer() {
  if (server && server.child) {
    return server;
  }
  const exe = procupinePath(String(config().get("path") || "").trim());
  log(`starting ${exe} serve`);
  server = new Server(exe, {
    onEvent: (event) => {
      if (event.event === "sample") {
        post({ type: "sample", event });
      } else if (event.event === "error") {
        log(`sampling failed: ${event.error}`);
      }
    },
    onExit: (why) => {
      log(why.message);
      columns = null;
      if (panel) {
        post({ type: "message", text: `${why.message}. Close and reopen the panel to start it again.` });
      }
    },
    log,
  });
  server.start();
  return server;
}

function post(message) {
  if (panel) {
    panel.webview.postMessage(message);
  }
}

async function init() {
  try {
    const s = ensureServer();
    columns = columns || (await s.request("columns"));
    post({
      type: "init",
      columns,
      platform: process.platform,
      user: os.userInfo().username,
      hexcavator: hexcavatorInstalled(),
      interval: config().get("interval") || 1000,
      serverPid: s.child ? s.child.pid : null,
    });
  } catch (e) {
    post({ type: "message", text: e.message });
    vscode.window.showErrorMessage(`Procupine: ${e.message}`);
  }
}

const VERBS = {
  kill: { title: "Kill", ing: "killing" },
  terminate: { title: "Terminate", ing: "terminating" },
  suspend: { title: "Suspend", ing: "suspending" },
  resume: { title: "Resume", ing: "resuming" },
};

async function action(m) {
  const verb = VERBS[m.action];
  if (!verb || !m.processes || !m.processes.length) {
    return;
  }
  const ps = m.processes;
  const destructive = m.action === "kill" || m.action === "terminate";
  if (destructive && config().get("confirmKill") !== false) {
    const what =
      ps.length === 1
        ? `"${ps[0].name}" (PID ${ps[0].pid})`
        : m.tree
          ? `"${ps[0].name}" (PID ${ps[0].pid}) and its ${ps.length - 1} descendant${ps.length > 2 ? "s" : ""}`
          : `${ps.length} processes`;
    const detail =
      m.action === "kill" ? "Killing a process ends it at once: what it hasn't saved is lost." : "The process is asked to end (SIGTERM); it may save its work, or ignore it.";
    const choice = await vscode.window.showWarningMessage(`${verb.title} ${what}?`, { modal: true, detail }, verb.title);
    if (choice !== verb.title) {
      return;
    }
  }
  try {
    const results = await ensureServer().request(m.action, { processes: ps.map((p) => p.id) });
    const failed = results.filter((r) => r.error);
    if (failed.length) {
      const byId = new Map(ps.map((p) => [p.id, p]));
      const first = failed[0];
      const p = byId.get(first.process);
      const name = p ? `${p.name} (${p.pid})` : `process ${first.process}`;
      const more = failed.length > 1 ? ` (and ${failed.length - 1} more)` : "";
      const hint = /denied|not permitted/i.test(first.error) ? " It may need administrator rights." : "";
      vscode.window.showErrorMessage(`Procupine: ${verb.ing} ${name}${more} failed: ${first.error}.${hint}`);
      for (const f of failed) {
        log(`${verb.ing} ${f.process}: ${f.error}`);
      }
    }
  } catch (e) {
    vscode.window.showErrorMessage(`Procupine: ${e.message}`);
  }
}

function html(webview, extensionUri) {
  const nonce = crypto.randomBytes(16).toString("base64");
  const uri = (name) => webview.asWebviewUri(vscode.Uri.joinPath(extensionUri, "media", name));
  return `<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="stylesheet" href="${uri("view.css")}">
<title>Processes</title></head><body>
<script nonce="${nonce}" src="${uri("model.js")}"></script>
<script nonce="${nonce}" src="${uri("view.js")}"></script>
</body></html>`;
}

function show(context, select) {
  if (panel) {
    panel.reveal();
    if (select) {
      post({ type: "select", pid: select });
    }
    return;
  }
  panel = vscode.window.createWebviewPanel("procupine.processes", "Processes", vscode.ViewColumn.Active, {
    enableScripts: true,
    retainContextWhenHidden: true,
    localResourceRoots: [vscode.Uri.joinPath(context.extensionUri, "media")],
  });
  panel.iconPath = vscode.Uri.joinPath(context.extensionUri, "media", "icon.svg");
  panel.webview.html = html(panel.webview, context.extensionUri);
  panel.webview.onDidReceiveMessage(async (m) => {
    switch (m.type) {
      case "ready":
        await init();
        if (select) {
          post({ type: "select", pid: select });
          select = null;
        }
        break;
      case "watch":
        ensureServer().request("watch", { interval: m.interval || 1000 }).catch((e) => log(e.message));
        break;
      case "pause":
        ensureServer().request("pause").catch((e) => log(e.message));
        break;
      case "action":
        await action(m);
        break;
      case "copy":
        await vscode.env.clipboard.writeText(String(m.text || ""));
        vscode.window.setStatusBarMessage("Copied", 1500);
        break;
      case "reveal":
        if (m.path) {
          await vscode.commands.executeCommand("revealFileInOS", vscode.Uri.file(m.path));
        }
        break;
      case "hexcavator":
        if (m.path) {
          await vscode.commands.executeCommand("hexcavator.open", vscode.Uri.file(m.path));
        }
        break;
    }
  });
  // Hidden: no samples (they cost a little CPU); shown again: from now.
  panel.onDidChangeViewState(() => {
    if (!server || !server.child) {
      return;
    }
    if (panel.visible) {
      server.request("watch", { interval: config().get("interval") || 1000 }).catch(() => {});
      server.request("refresh").catch(() => {});
    } else {
      server.request("pause").catch(() => {});
    }
  });
  panel.onDidDispose(() => {
    panel = null;
    if (server) {
      server.stop();
      server = null;
    }
    columns = null;
  });
}

function activate(context) {
  channel = vscode.window.createOutputChannel("Procupine");
  context.subscriptions.push(
    channel,
    vscode.commands.registerCommand("procupine.show", () => show(context)),
    vscode.commands.registerCommand("procupine.showLog", () => channel.show(true)),
    vscode.commands.registerCommand("procupine.findProcess", async () => {
      const pid = await vscode.window.showInputBox({ prompt: "The process ID to show", validateInput: (v) => (/^\d+$/.test(v.trim()) ? null : "A process ID") });
      if (pid) {
        show(context, Number(pid.trim()));
      }
    }),
    { dispose: () => server && server.stop() }
  );
}

function deactivate() {
  if (server) {
    server.stop();
    server = null;
  }
}

module.exports = { activate, deactivate };
