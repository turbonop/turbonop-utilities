"""Folds every static library under the given directories with turbolib, as
a build would (`lib /OUT:x.lib in.lib`, the library's members as inputs),
and checks each result.

    python everylib.py <dir or .lib>... [--turbolib PATH] [--jobs N]
                       [--options "--fold=identical --precomp=library"]

Each library is remade by lib.exe (`/Brepro`), by turbolib with
`--fold=none` (which must be the same bytes: listed apart if it isn't),
and by turbolib with the options and `--strict` (so a fold that fails,
which a build would get unfolded with a warning, is an error here), with
turbolib's self-check on; then `turbolib verify <lib.exe's> <the folded>`
checks the result from outside. (Against lib.exe's remaking, not the
library itself: some of Microsoft's libraries have indexes lib.exe doesn't
make from their members.) A library turbolib refuses is listed apart, with
lib.exe's verdict on the same command. Prints a summary, every failure, and exits
with 1 if any library failed.

Runs at below-normal priority, a few libraries at a time (each turbolib
uses every core). Results go to a scratch directory, deleted at the end.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
LOW = getattr(subprocess, "BELOW_NORMAL_PRIORITY_CLASS", 0)
TIMEOUT = 900


def libraries(paths):
    out = []
    for p in paths:
        if os.path.isdir(p):
            for d, _, files in os.walk(p):
                out += [os.path.join(d, f) for f in sorted(files) if f.lower().endswith(".lib")]
        else:
            out.append(p)
    return sorted(out)


def lib_exe():
    vswhere = os.path.join(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
                           "Microsoft Visual Studio", "Installer", "vswhere.exe")
    vs = subprocess.run([vswhere, "-latest", "-property", "installationPath"], capture_output=True,
                        text=True, timeout=60).stdout.strip()
    tools = os.path.join(vs, "VC", "Tools", "MSVC")
    version = sorted(os.listdir(tools))[-1]
    return os.path.join(tools, version, "bin", "Hostx64", "x64", "lib.exe")


def one(i, lib, a, scratch):
    """The verdict on one library, and its details."""
    out = os.path.join(scratch, f"{i}.lib")
    ref = os.path.join(scratch, f"{i}.ref.lib")
    plain = os.path.join(scratch, f"{i}.plain.lib")

    def run(args, options=None):
        env = dict(os.environ)
        env.pop("TURBOLIB_OPTIONS", None)
        if options is not None:
            env["TURBOLIB_OPTIONS"] = options
        p = subprocess.run(args, capture_output=True, text=True, errors="replace", env=env,
                           timeout=TIMEOUT, creationflags=LOW)
        return p.returncode, p.stdout + p.stderr

    try:
        # lib.exe's own remaking of the library: what turbolib stands in for.
        rc_ref, text_ref = run([a.lib_exe, "/nologo", "/Brepro", f"/OUT:{ref}", lib])
        rc_plain, text_plain = run([a.turbolib, "lib", "/nologo", "/Brepro", f"/OUT:{plain}", lib],
                                   "--fold=none")
        rc, text = run([a.turbolib, "lib", "/nologo", "/Brepro", f"/OUT:{out}", lib],
                       a.options + " --strict")
        if rc != 0:
            if "LNK1000" in text or "panicked" in text:
                return lib, "fold failed", text.strip().splitlines()[-1:]
            if rc == rc_ref:
                return lib, "refused, as lib.exe refuses it", text.strip().splitlines()[-1:]
            return lib, "refused, though lib.exe takes it", text[-2000:]
        if rc_ref != 0:
            return lib, "taken, though lib.exe refuses it", text_ref.strip().splitlines()[-1:]
        same = rc_plain == 0 and open(plain, "rb").read() == open(ref, "rb").read()
        v = subprocess.run([a.turbolib, "verify", ref, out], capture_output=True, text=True,
                           errors="replace", timeout=TIMEOUT, creationflags=LOW)
        sizes = (os.path.getsize(ref), os.path.getsize(out))
        if v.returncode != 0:
            fails = [l for l in v.stdout.splitlines() if l.startswith("FAILED")]
            verdict = "verify failed" + ("" if same else " (and --fold=none isn't lib.exe's)")
            return lib, verdict, "\n".join(fails) + "\n" + v.stdout[-1500:]
        if not same:
            return lib, "ok, but --fold=none isn't lib.exe's bytes", sizes
        return lib, "ok", sizes
    except subprocess.TimeoutExpired:
        return lib, "timeout", ""
    finally:
        for f in (out, ref, plain):
            if os.path.exists(f):
                os.remove(f)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--turbolib", default=os.path.join(ROOT, "target", "release", "turbolib.exe"))
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--options", default="--fold=identical --precomp=library")
    a = ap.parse_args()
    a.turbolib = os.path.abspath(a.turbolib)
    a.lib_exe = lib_exe()
    libs = libraries(a.paths)
    scratch = tempfile.mkdtemp(prefix="everylib-")
    results = {}
    before = after = 0
    try:
        with ThreadPoolExecutor(a.jobs) as pool:
            for lib, verdict, detail in pool.map(lambda x: one(x[0], x[1], a, scratch),
                                                 enumerate(libs)):
                results.setdefault(verdict, []).append((lib, detail))
                if verdict.startswith("ok"):
                    before += detail[0]
                    after += detail[1]
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    print(f"{len(libs)} libraries, options {a.options!r}")
    for verdict, items in sorted(results.items()):
        print(f"  {verdict}: {len(items)}")
    if before:
        print(f"  the ok ones: {before / 2**20:.1f} MiB to {after / 2**20:.1f} MiB "
              f"({100 * (1 - after / before):.2f}% smaller)")
    bad = 0
    for verdict, items in sorted(results.items()):
        if verdict.startswith("ok") or verdict == "refused, as lib.exe refuses it":
            if not verdict.startswith("ok"):
                for lib, detail in items:
                    print(f"{verdict}: {lib}: {detail}")
            continue
        for lib, detail in items:
            bad += 1
            print(f"== {verdict}: {lib}\n{detail}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
