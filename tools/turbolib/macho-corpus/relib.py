#!/usr/bin/env python3
"""Remakes every archive of the Mach-O corpus (build.py's) by the same
commands with turbolib in place of llvm-ar, llvm-ranlib, and
llvm-libtool-darwin, and compares the results with the corpus's, byte for
byte. With --options, turbolib runs with TURBOLIB_OPTIONS (folding: the
results then differ, and are kept for relink.py and verify).

    relib.py --turbolib PATH [--out DIR] [--options TEXT] [--time] [--jobs N]

DIR (default $CORPUS/macho/relib) receives lib/<kind>/<arch>/lib*.a. --time
runs each archive's commands one after another, real tools then turbolib,
best of two, and reports the totals. Runs at low priority.
"""

import argparse
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--turbolib", required=True)
    ap.add_argument("--out", default=str(build.OUT / "relib"))
    ap.add_argument("--options")
    ap.add_argument("--time", action="store_true")
    ap.add_argument("--largest", type=int, default=0, help="only the N largest libraries")
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    a = ap.parse_args()
    out = Path(a.out)
    tools = out / "bin"
    tools.mkdir(parents=True, exist_ok=True)
    for n in ("llvm-ar", "llvm-ranlib", "llvm-libtool-darwin"):
        p = tools / n
        if p.is_symlink() or p.exists():
            p.unlink()
        p.symlink_to(os.path.abspath(a.turbolib))
    libs, _ = build.ninja_libs()
    wanted = sorted(f"lib/{p.name}" for p in (build.OUT / "lib" / "llvm-ar" / "arm64").glob("*.a"))
    if a.largest:
        wanted = sorted(wanted, key=lambda w: -(build.OUT / "lib" / "llvm-ar" / "arm64" / Path(w).name).stat().st_size)[:a.largest]
    ours = build.archive_jobs(wanted, libs, tools, out / "lib")
    env = dict(os.environ)
    if a.options:
        env["TURBOLIB_OPTIONS"] = a.options

    def run(cmds, e):
        for c in cmds:
            r = subprocess.run(build.NICE + c, capture_output=True, text=True, env=e)
            if r.returncode != 0:
                return f"{' '.join(c[:3])}: {r.stderr.strip()[:300]}"
        return None

    if a.time:
        real = build.archive_jobs(wanted, libs, build.LLVM_BIN, out / "timing-real")
        best = {}
        for which, jobs, e in (("real", real, os.environ), ("turbolib", ours, env)):
            for _ in range(2):
                t = time.monotonic()
                for _, _, cmds in jobs:
                    run(cmds, e)
                best[which] = min(best.get(which, 1e9), time.monotonic() - t)
        size = sum((build.OUT / "lib" / k / Path(p).name).stat().st_size for k, p, _ in
                   [(k, p, c) for k, p, c in real])
        print(f"{len(real)} archives, {size / 2**30:.2f} GiB: real tools {best['real']:.2f} s, "
              f"turbolib {best['turbolib']:.2f} s, {best['turbolib'] / best['real']:.2f}x")
        return

    with ThreadPoolExecutor(a.jobs) as ex:
        errors = list(ex.map(lambda j: run(j[2], env), ours))
    same = differ = failed = 0
    total = 0
    for (kind, path, _), err in zip(ours, errors):
        orig = build.OUT / "lib" / kind / path.name
        if err:
            failed += 1
            print(f"FAIL {kind}/{path.name}: {err}")
            continue
        a_, b_ = orig.read_bytes(), path.read_bytes()
        total += len(a_)
        if a_ == b_:
            same += 1
        else:
            differ += 1
            if not a.options:
                at = next((k for k in range(min(len(a_), len(b_))) if a_[k] != b_[k]), min(len(a_), len(b_)))
                print(f"DIFF {kind}/{path.name}: at {at} ({len(a_)} vs {len(b_)} bytes)")
    print(f"{len(ours)} archives ({total / 2**30:.2f} GiB): {same} the same, {differ} different, {failed} failed")


if __name__ == "__main__":
    main()
