"""Renders media/view.js in headless Edge with real samples and checks the
result: `python harness.py <procupine> [shots]` (with a directory `shots`,
it also saves screenshots there, to look at).

Records a few samples from `procupine serve`, builds a page that loads the
view and posts it the samples as the extension does, runs a script against
it (a click, a filter, a key), then dumps the DOM and checks what was drawn.
Edge's processes are ended after each run (they are found, and killed, with
procupine itself).
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
MEDIA = os.path.join(os.path.dirname(HERE), "media")
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"

THEME = """
:root {
  --vscode-font-family: "Segoe UI", system-ui, sans-serif;
  --vscode-font-size: 13px;
  --vscode-foreground: #cccccc;
  --vscode-descriptionForeground: #9d9d9d;
  --vscode-editor-background: #1f1f1f;
  --vscode-sideBar-background: #181818;
  --vscode-panel-border: #2b2b2b;
  --vscode-textLink-foreground: #4daafc;
  --vscode-focusBorder: #0078d4;
  --vscode-list-activeSelectionBackground: #04395e;
}
"""

VSCODE_STUB = """
window.__posted = [];
window.acquireVsCodeApi = function () {
  return {
    postMessage: function (m) {
      window.__posted.push(m);
      var p = document.getElementById("posted");
      if (p) p.textContent = JSON.stringify(window.__posted);
    },
    getState: function () { return null; },
    setState: function () {},
  };
};
"""


def record(exe, count=3, interval=300):
    """`count` sample events from `procupine serve`, and its columns and PID."""
    p = subprocess.Popen([exe, "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    p.stdin.write(json.dumps({"id": 1, "method": "columns"}) + "\n")
    p.stdin.write(json.dumps({"id": 2, "method": "watch", "interval": interval}) + "\n")
    p.stdin.flush()
    columns, samples = None, []
    while len(samples) < count:
        v = json.loads(p.stdout.readline())
        if v.get("id") == 1:
            columns = v["result"]
        elif v.get("event") == "sample":
            samples.append(v)
    p.stdin.close()
    p.wait(timeout=30)
    return columns, samples, p.pid


def page(init, samples, then=""):
    def read(name):
        with open(os.path.join(MEDIA, name), encoding="utf-8") as f:
            return f.read()

    posts = "".join(f"window.postMessage({json.dumps({'type': 'sample', 'event': s})}, '*');\n" for s in samples)
    return f"""<!DOCTYPE html><html><head><meta charset="utf-8"><style>{THEME}{read("view.css")}</style></head><body>
<script>{VSCODE_STUB}</script>
<script>{read("model.js")}</script>
<script>{read("view.js")}</script>
<pre id="posted" hidden></pre>
<script>
window.postMessage({json.dumps(init)}, "*");
{posts}
setTimeout(function () {{ {then} }}, 300);
</script>
</body></html>"""


def end_edge(profile):
    """Ends the Edge processes that ran with `profile`, by procupine."""
    out = subprocess.run([EXE, "list", "-json", "-interval", "0", "-filter", "msedge"], capture_output=True, text=True, encoding="utf-8")
    try:
        procs = json.loads(out.stdout)["processes"]
    except (ValueError, KeyError):
        return
    keys = [p["key"] for p in procs if profile.lower() in (p.get("commandLine") or "").lower()]
    if keys:
        subprocess.run([EXE, "kill", *keys], capture_output=True)


def render(html, shot=None, size="1400,900"):
    with tempfile.TemporaryDirectory() as tmp:
        f = os.path.join(tmp, "page.html")
        with open(f, "w", encoding="utf-8") as out:
            out.write(html)
        profile = os.path.join(tmp, "profile")
        url = "file:///" + f.replace("\\", "/")
        common = [EDGE, "--headless", "--disable-gpu", "--hide-scrollbars", f"--user-data-dir={profile}", f"--window-size={size}", "--virtual-time-budget=3000"]
        try:
            if shot:
                subprocess.run(common + [f"--screenshot={shot}", url], capture_output=True, timeout=120)
            dom = subprocess.run(common + ["--dump-dom", url], capture_output=True, text=True, encoding="utf-8", timeout=120).stdout
        finally:
            end_edge(profile)
            time.sleep(0.2)
        return dom


FAILURES = []


def check(cond, what):
    if not cond:
        FAILURES.append(what)
        print(f"FAIL: {what}")


def rows_of(dom):
    """The rows drawn: (classes, id, name)."""
    found = []
    for m in re.finditer(r'<div class="(row[^"]*)" data-id="(\d+)" data-i="\d+">(.*?)</div></div>(?=<div class="row|</div>)', dom, re.S):
        name = re.search(r'<span class="label">(.*?)</span>', m.group(3))
        found.append((m.group(1).split(), int(m.group(2)), name.group(1) if name else None))
    return found


def main():
    global EXE
    EXE = os.path.abspath(sys.argv[1])
    shots = sys.argv[2] if len(sys.argv) > 2 else None
    if shots:
        os.makedirs(shots, exist_ok=True)
    columns, samples, server_pid = record(EXE)
    me = os.getpid()
    init = {"type": "init", "columns": columns, "platform": sys.platform if sys.platform != "win32" else "win32", "user": os.environ.get("USERNAME") or os.environ.get("USER"), "hexcavator": True, "interval": 1000, "serverPid": server_pid}
    names = {p["id"]: p["name"] for s in samples for p in s["added"]}
    mine = next((p for p in samples[0]["added"] if p["pid"] == me), None)
    check(mine is not None, "this harness's process is in the samples")

    # The tree, as it opens.
    dom = render(page(init, samples), os.path.join(shots, "tree.png") if shots else None)
    rows = rows_of(dom)
    check(len(rows) > 10, f"rows drawn ({len(rows)})")
    check(all(n in names.values() for _, _, n in rows if n), "rows are processes")
    check('class="h r" data-key="cpu"' in dom, "the CPU column's header")
    check("processes</span>" in dom, "the system's counts")
    check("CPU history" in dom and "<polyline" in dom, "CPU sparklines")
    check('"type":"ready"' in dom and '"type":"watch"' in dom, "the view asked to watch")

    # Flat, sorted by CPU (the header clicked): largest first.
    dom = render(page(init, samples, 'document.getElementById("tree").click(); document.querySelector(\'[data-key="cpu"]\').click();'))
    check('class="arrow">▾' in dom, "sorted by CPU, descending")
    last = samples[-1]
    cpu = {r[0]: r[2] for s in samples for r in s["rows"]}
    shown = [cpu.get(i, 0) for _, i, _ in rows_of(dom)]
    check(shown == sorted(shown, reverse=True), "rows in CPU order")

    # Filtered to this process: it, and its ancestors (dimmed).
    name = mine["name"] if mine else "python"
    script = f'var f = document.getElementById("filter"); f.value = {json.dumps(str(me))}; f.dispatchEvent(new Event("input"));'
    dom = render(page(init, samples, script), os.path.join(shots, "filter.png") if shots else None)
    rows = rows_of(dom)
    check(any(n == name for _, _, n in rows), f"the filter shows {name}")
    check(all("nomatch" in c for c, _, n in rows if n != name), "ancestors dimmed")

    # Selected (a click): the details, with its command line; then Delete
    # posts a kill of it.
    select = f"""
      var r = [].find.call(document.querySelectorAll('.row'), function (e) {{ return e.dataset.id == {mine["id"] if mine else 0}; }});
      if (r) r.dispatchEvent(new MouseEvent('mousedown', {{bubbles: true, button: 0}}));
      document.getElementById('table').dispatchEvent(new KeyboardEvent('keydown', {{key: 'Delete', bubbles: true}}));
    """
    dom = render(page(init, samples, script + select), os.path.join(shots, "details.png") if shots else None)
    check("<dt>Command line</dt>" in dom, "the details' command line")
    check("Open in Hexcavator" in dom, "the details offer Hexcavator")
    posted = re.search(r'<pre id="posted"[^>]*>(.*?)</pre>', dom, re.S)
    msgs = json.loads(posted.group(1).replace("&quot;", '"').replace("&amp;", "&")) if posted and posted.group(1) else []
    kills = [m for m in msgs if m.get("type") == "action"]
    check(kills and kills[0]["action"] == "kill" and kills[0]["processes"][0]["pid"] == me, f"Delete asks to kill the selected ({kills})")

    # The context menu.
    # (The rows are drawn again once one is selected: the row, found again.)
    menu = select.replace(
        "document.getElementById('table').dispatchEvent(new KeyboardEvent('keydown', {key: 'Delete', bubbles: true}));",
        "r = document.querySelector('.row.sel'); if (r) r.dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, clientX: 200, clientY: 200}));",
    )
    dom = render(page(init, samples, script + menu), os.path.join(shots, "menu.png") if shots else None)
    check("Kill Process Tree" in dom and "Copy Command Line" in dom, "the context menu")

    if FAILURES:
        print(f"{len(FAILURES)} failed")
        sys.exit(1)
    print("harness: ok")


if __name__ == "__main__":
    main()
