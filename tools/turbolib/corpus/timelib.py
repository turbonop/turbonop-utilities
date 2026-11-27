"""Times archivers on a Ninja build's own archiver commands.

    python timelib.py <build dir> <tool>... [--largest N] [--rounds R]
                      [--env NAME=VALUE]...

Each tool is run, one library at a time (nothing else running), on the
build's command for each of the N largest libraries the build made, R times;
the best of the R is kept for each library and tool. Inputs are read once
first, so the times are with a warm file cache. A tool may be written
`label=path` to name it in the report, and `label=path;NAME=VALUE` to run it
with an environment variable set (for TURBOLIB_OPTIONS).

The outputs go to a scratch directory and are deleted.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import relib  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("build_dir")
    ap.add_argument("tools", nargs="+")
    ap.add_argument("--largest", type=int, default=20)
    ap.add_argument("--rounds", type=int, default=3)
    a = ap.parse_args()
    rules = relib.read_rules(os.path.join(a.build_dir, "CMakeFiles", "rules.ninja"))
    libs = [l for l in relib.read_libraries(a.build_dir, rules)
            if os.path.exists(os.path.join(a.build_dir, l["out"]))]
    libs.sort(key=lambda l: -os.path.getsize(os.path.join(a.build_dir, l["out"])))
    libs = libs[:a.largest]
    total = sum(os.path.getsize(os.path.join(a.build_dir, l["out"])) for l in libs)
    tools = []
    for t in a.tools:
        label, _, rest = t.partition("=") if "=" in t.split(";")[0] else (t, "", t)
        path, _, env = rest.partition(";")
        environ = dict(os.environ)
        if env:
            k, _, v = env.partition("=")
            environ[k] = v
        tools.append((label or os.path.basename(path), path, environ))
    scratch = tempfile.mkdtemp(prefix="timelib-")
    # Warm the cache: read every input once.
    for l in libs:
        for p in l["in"]:
            full = os.path.join(a.build_dir, p)
            if os.path.exists(full):
                with open(full, "rb") as f:
                    while f.read(1 << 24):
                        pass
    best = {label: [] for label, _, _ in tools}
    for l in libs:
        for label, path, environ in tools:
            times = []
            for _ in range(a.rounds):
                out = os.path.join(scratch, "out.lib")
                rsp = out + ".rsp"
                args, content = relib.archiver_args(l, rules, out, rsp)
                if content is not None:
                    with open(rsp, "w", encoding="utf-8") as f:
                        f.write(content)
                t0 = time.perf_counter()
                p = subprocess.run([path] + args, cwd=a.build_dir, capture_output=True, env=environ)
                times.append(time.perf_counter() - t0)
                if p.returncode != 0:
                    print(f"{label} failed on {l['out']}: {p.stdout.decode(errors='replace')[:300]}")
                    sys.exit(1)
                os.remove(out)
            best[label].append(min(times))
    print(f"{len(libs)} largest libraries, {total / 2**30:.2f} GiB, best of {a.rounds}, warm cache")
    base = sum(best[tools[0][0]])
    print(f"{'library':48} " + " ".join(f"{label:>12}" for label, _, _ in tools))
    for i, l in enumerate(libs):
        print(f"{os.path.basename(l['out']):48} " + " ".join(f"{best[label][i]:>11.2f}s" for label, _, _ in tools))
    print(f"{'total':48} " + " ".join(f"{sum(best[label]):>11.2f}s" for label, _, _ in tools))
    print(f"{'against the first':48} " + " ".join(f"{sum(best[label]) / base:>11.2f}x" for label, _, _ in tools))
    shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    main()
