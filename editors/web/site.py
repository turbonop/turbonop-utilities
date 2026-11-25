"""Assembles the web version's site (GitHub Pages serves it as it is):

    python editors/web/site.py --wasm <hexcavator.wasm> --codicons <dir> --out <dir>

  <out>/index.html, app.js, app.css, sw.js, host/, wasi/   editors/web
  <out>/ext/                     the extension's code and media
                                 (editors/vscode/hexcavator)
  <out>/hexcavator.wasm          hexcavator for wasm32-wasip1-threads
  <out>/codicons/                VS Code's icon font (@vscode/codicons' dist)
  <out>/LICENSE, NOTICE, THIRD_PARTY_NOTICES.txt, codicons/LICENSE*

(Not the tests, nor serve.py.)
"""

import argparse
import os
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
EXT = os.path.join(ROOT, "editors", "vscode", "hexcavator")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--wasm", required=True)
    ap.add_argument("--codicons", required=True, help="@vscode/codicons' package directory (with dist/ and its licenses)")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    out = a.out
    if os.path.exists(out):
        shutil.rmtree(out)
    os.makedirs(out)
    for name in ["index.html", "app.js", "app.css", "sw.js"]:
        shutil.copy2(os.path.join(HERE, name), out)
    for d in ["host", "wasi"]:
        shutil.copytree(os.path.join(HERE, d), os.path.join(out, d))
    ext = os.path.join(out, "ext")
    os.makedirs(ext)
    for name in ["extension.js", "sidebar.js", "package.json"]:
        shutil.copy2(os.path.join(EXT, name), ext)
    for d in ["lib", "media"]:
        shutil.copytree(os.path.join(EXT, d), os.path.join(ext, d))
    shutil.copy2(a.wasm, os.path.join(out, "hexcavator.wasm"))
    cod = os.path.join(out, "codicons")
    dist = os.path.join(a.codicons, "dist")
    os.makedirs(cod)
    for name in ["codicon.css", "codicon.ttf"]:
        shutil.copy2(os.path.join(dist, name), cod)
    for name in ["LICENSE", "LICENSE-CODE"]:
        if os.path.exists(os.path.join(a.codicons, name)):
            shutil.copy2(os.path.join(a.codicons, name), cod)
    for p in ["LICENSE", "NOTICE", os.path.join("apps", "hexcavator", "THIRD_PARTY_NOTICES.txt")]:
        shutil.copy2(os.path.join(ROOT, p), out)
    # (No Jekyll: Pages serves the files as they are.)
    open(os.path.join(out, ".nojekyll"), "w").close()
    total = sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(out) for f in fs)
    print(f"{out}: {total / 1048576:.1f} MiB")


if __name__ == "__main__":
    sys.exit(main())
