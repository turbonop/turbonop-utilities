"""The web version's tests, in a headless browser (Edge or Chrome):

    python editors/web/test/run.py --wasm <hexcavator.wasm> [--browser <exe>]
                                   [--codicons <dir>]

runtime.html: hexcavator serve, as the page runs it (WebAssembly, its
threads Web Workers), answering requests on a test binary; app.html: the
whole page, as a person uses it (the sidebar, the Symbols page, a text
tool, a row's context menu, disassembly). Each page posts what it found
to the server (serve.py's /__report); this checks it.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile

# (What a failure shows is the page's text, not all of it in the console's
# code page.)
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

HERE = os.path.dirname(os.path.abspath(__file__))
SERVE = os.path.join(HERE, "..", "serve.py")

BROWSERS = [
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "google-chrome",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
]


def browser(given):
    for b in [given] + BROWSERS if given else BROWSERS:
        if b and (os.path.isfile(b) or shutil.which(b)):
            return b if os.path.isfile(b) else shutil.which(b)
    sys.exit("no browser found: give one with --browser")


def run(page, args, port, timeout):
    """The page's report (a dict), run in the browser with the server."""
    server = subprocess.Popen(
        [sys.executable, SERVE, "--wasm", args.wasm, "--port", str(port), "--reports", "1"]
        + (["--codicons", args.codicons] if args.codicons else []),
        stdout=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    first = server.stdout.readline()
    assert first.startswith("serving"), first
    profile = tempfile.mkdtemp(prefix="hexcavator-web-")
    b = subprocess.Popen(
        [args.browser, "--headless=new", "--disable-gpu", "--no-first-run", "--no-sandbox", f"--user-data-dir={profile}", "--window-size=1400,900", f"http://127.0.0.1:{port}/{page}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        out, _ = server.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        server.kill()
        out, _ = server.communicate()
        out += '\n{"error": "timed out"}'
    finally:
        b.kill()
        b.wait()
        shutil.rmtree(profile, ignore_errors=True)
    lines = [l for l in out.splitlines() if l.startswith("{")]
    return json.loads(lines[-1]) if lines else {"error": "no report: " + out[-500:]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--wasm", required=True)
    ap.add_argument("--browser")
    ap.add_argument("--codicons")
    args = ap.parse_args()
    args.browser = browser(args.browser)
    failures = []

    def check(name, ok, detail):
        print(("ok   " if ok else "FAIL ") + name)
        if not ok:
            failures.append(name)
            print("     " + json.dumps(detail, ensure_ascii=False)[:2000])

    r = run("test/runtime.html", args, 8765, 180)
    check("runtime: no error", "error" not in r and "timeout" not in r, r)
    check("runtime: info", r.get("infoCode") == 0 and "PE32+ image, x64" in r.get("info", ""), r)
    check("runtime: size (with its PDB)", r.get("sizeCode") == 0 and "PDB: /files/xref-x64.pdb" in r.get("size", ""), r)
    check("runtime: xref", r.get("xrefCode") == 0 and "dead_entry(int) calls it" in r.get("xref", ""), r)

    r = run("test/app.html", args, 8766, 300)
    check("app: no error", "error" not in r, r)
    check("app: cross-origin isolated", r.get("isolated") is True, r)
    tree = r.get("tree", [])
    check("app: the sidebar's entry and its tools", bool(tree) and tree[0].startswith("xref-x64.exe") and "Symbols" in tree and "Headers" in tree, tree)
    check("app: the Symbols page", "main(int, char**)" in r.get("symbols", "") and "447 rows" in r.get("symbols", ""), r.get("symbols", "")[:500])
    check("app: a text tool's output", "FILE HEADER VALUES" in r.get("text", ""), r.get("text"))
    check("app: a row's context menu", "Disassemble" in r.get("menu", []) and "Why Is This Here?" in r.get("menu", []), r.get("menu"))
    check("app: disassembly", "push" in r.get("disasm", ""), r.get("disasm"))
    check("app: no error notifications", not r.get("toasts"), r.get("toasts"))
    if failures:
        sys.exit(f"{len(failures)} failed")
    print("all passed")


if __name__ == "__main__":
    main()
