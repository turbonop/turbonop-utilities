#!/usr/bin/env python3
"""Remakes every static library of a corpus build with turbolib, running the
build's own archiver commands (from `ninja -t commands`) with turbolib in
the archiver's place, and checks each comes out byte for byte as the
build's.

    relib.py --build ~/tlcorpus/build-gcc --turbolib PATH [--out DIR] [-j N] [--time] [--top N]

CMake's commands for a static library are `cmake -E rm -f lib.a`, then
`ar qc lib.a objects...` (or `llvm-ar`), then `ranlib lib.a` (or
`llvm-ranlib`). Each is rerun from the build directory with the library
written under --out (default: <build>/relib) instead, turbolib run as
`ar`, `ranlib`, `llvm-ar`, or `llvm-ranlib` (a link of that name to it),
and the result compared with the build's library.

--time runs instead each library's commands with the build's own archiver
and with turbolib, best of 2 (warm cache), on the --top N largest
libraries, and prints the totals (timelib.py's measure, for ELF).
"""

import argparse
import concurrent.futures
import os
import re
import shlex
import subprocess
import sys
import tempfile
import time

ARCHIVERS = ("ar", "ranlib", "llvm-ar", "llvm-ranlib")


def library_commands(build):
    """Each static library's commands: (library path, [argv, ...])."""
    text = subprocess.run(
        ["ninja", "-C", build, "-t", "commands"], capture_output=True, text=True, check=True
    ).stdout
    out = {}
    for line in text.splitlines():
        if " qc " not in line and " rcs " not in line:
            continue
        parts = [p.strip() for p in line.split("&&")]
        cmds = []
        lib = None
        for p in parts:
            if not p or p == ":":
                continue
            argv = shlex.split(p)
            tool = os.path.basename(argv[0])
            if tool in ARCHIVERS:
                cmds.append(argv)
                if tool in ("ar", "llvm-ar") and lib is None:
                    lib = argv[2]
        if lib and lib.endswith(".a"):
            out[lib] = cmds
    return out


def retarget(cmds, lib, newlib, tools):
    """The commands writing `newlib` instead of `lib`, each archiver
    replaced by the one of the same name in `tools` (None: the build's)."""
    res = []
    for argv in cmds:
        argv = [newlib if a == lib else a for a in argv]
        if tools:
            argv = [os.path.join(tools, os.path.basename(argv[0]))] + argv[1:]
        res.append(argv)
    return res


def run(cmds, cwd):
    for argv in cmds:
        p = subprocess.run(argv, cwd=cwd, capture_output=True)
        if p.returncode != 0:
            return f"{argv[0]} exited {p.returncode}: {p.stderr.decode(errors='replace')[:300]}"
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", required=True)
    ap.add_argument("--turbolib", required=True)
    ap.add_argument("--out")
    ap.add_argument("-j", type=int, default=os.cpu_count())
    ap.add_argument("--time", action="store_true")
    ap.add_argument("--top", type=int, default=20)
    ap.add_argument("-v", action="store_true")
    args = ap.parse_args()
    build = os.path.abspath(args.build)
    out = os.path.abspath(args.out or os.path.join(build, "relib"))
    os.makedirs(out, exist_ok=True)
    tools = tempfile.mkdtemp(prefix="relib-tools-")
    for n in ARCHIVERS:
        os.symlink(os.path.abspath(args.turbolib), os.path.join(tools, n))
    libs = library_commands(build)
    if not libs:
        sys.exit("no static libraries' commands found")

    if args.time:
        sized = sorted(libs, key=lambda l: -os.path.getsize(os.path.join(build, l)))[: args.top]
        totals = {"build": 0.0, "turbolib": 0.0}
        size = 0
        for lib in sized:
            size += os.path.getsize(os.path.join(build, lib))
            for who, t in (("build", None), ("turbolib", tools)):
                best = None
                for _ in range(2):
                    target = os.path.join(out, who + "-" + os.path.basename(lib))
                    if os.path.exists(target):
                        os.remove(target)
                    start = time.perf_counter()
                    err = run(retarget(libs[lib], lib, target, t), build)
                    took = time.perf_counter() - start
                    if err:
                        sys.exit(f"{lib}: {who}: {err}")
                    best = took if best is None else min(best, took)
                totals[who] += best
            if args.v:
                print(lib)
        print(f"{len(sized)} largest libraries, {size / 2**30:.2f} GiB:")
        print(f"  build's archiver: {totals['build']:.2f} s")
        print(f"  turbolib:         {totals['turbolib']:.2f} s ({totals['turbolib'] / totals['build']:.2f}x)")
        return

    def one(lib):
        target = os.path.join(out, lib.replace("/", "_"))
        if os.path.exists(target):
            os.remove(target)
        err = run(retarget(libs[lib], lib, target, tools), build)
        if err:
            return lib, err
        with open(os.path.join(build, lib), "rb") as f:
            want = f.read()
        with open(target, "rb") as f:
            got = f.read()
        os.remove(target)
        if want == got:
            return lib, None
        at = next((i for i in range(min(len(want), len(got))) if want[i] != got[i]), min(len(want), len(got)))
        return lib, f"differs at {at} ({len(want)} vs {len(got)} bytes)"

    same = 0
    total_size = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.j)) as pool:
        for lib, err in pool.map(one, sorted(libs)):
            total_size += os.path.getsize(os.path.join(build, lib))
            if err:
                print(f"{lib}: {err}")
            else:
                same += 1
                if args.v:
                    print(f"{lib}: the same")
    print(f"{same} of {len(libs)} libraries remade byte for byte ({total_size / 2**30:.2f} GiB)")
    sys.exit(0 if same == len(libs) else 1)


if __name__ == "__main__":
    main()
