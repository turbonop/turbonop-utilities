#!/usr/bin/env python3
"""A small Mach-O fixture for folding, built from sources this script writes:
two libraries sharing inline functions, templates, vtables, a static local,
a TLS variable, exceptions, and static initializers (members with side
effects first, in between, and last), each folded with turbolib, and a
program linked with ld64.lld against the originals and against the folded
ones in both library orders, compared as relink.py compares (members
loaded in order, then `turbolib compare-images` of the two images with
their dSYMs: code, data, symbols, initializers in order, unwind
information, imports, DWARF).

    fixture.py --turbolib PATH [--keep]

For each of arm64 and x86_64, -O0 and -O2. Checks `turbolib verify` of each
folded library too.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build  # noqa: E402
import relink  # noqa: E402

B = build.LLVM_BIN

COMMON = r"""
#include <cstddef>
struct Shape { virtual ~Shape() {} virtual int area() const = 0; virtual int sides() const { return 0; } };
struct Square : Shape { int s; explicit Square(int s) : s(s) {} int area() const override { return s * s; } int sides() const override { return 4; } };
template <class T> T twice(T x) { return x + x; }
template <class T> struct Box { static T value; T get() const { return value + twice(value); } };
template <class T> T Box<T>::value = 7;
inline int counter() { static int n = 0; return ++n; }
inline thread_local int tls_hits = 0;
inline int checked(int x) { if (x < 0) throw x; return twice(x); }
inline int guarded(int x) { try { return checked(x); } catch (int e) { return -e; } }
extern int log_init(const char *who);
"""

SOURCES = {
    "common.h": COMMON,
    "a1.cpp": '#include "common.h"\nint a1(int x) { Square s(x); return s.area() + Box<int>().get() + counter() + guarded(x); }\n',
    "a2.cpp": '#include "common.h"\nstatic int init_a2 = log_init("a2");\n'
              'int a2(int x) { Square s(x); tls_hits++; return s.sides() + Box<long>().get() + twice(x) + init_a2; }\n',
    "a3.cpp": '#include "common.h"\nint a3(int x) { return Box<int>().get() + twice(x) + counter() + guarded(-x); }\n',
    "b1.cpp": '#include "common.h"\nstatic int init_b1 = log_init("b1");\n'
              'int b1(int x) { Square s(x); return s.area() + Box<int>().get() + init_b1; }\n',
    "b2.cpp": '#include "common.h"\nint b2(int x) { return twice(x) + Box<long>().get() + counter() + guarded(x); }\n',
    "b3.cpp": '#include "common.h"\nstatic int init_b3 = log_init("b3");\nint b3(int x) { return twice(x) + init_b3; }\n',
    "main.cpp": '#include "common.h"\n#include <cstdio>\nint a1(int); int a2(int); int a3(int); int b1(int); int b2(int);\n'
                'int log_init(const char *w) { std::printf("init %s\\n", w); return 1; }\n'
                'int main(int c, char **) { return a1(c) + a2(c) + a3(c) + b1(c) + b2(c); }\n',
}


def run(cmd, cwd, **kw):
    return subprocess.run(build.NICE + cmd, cwd=cwd, check=True, capture_output=True, text=True, **kw)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--turbolib", required=True)
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    top = tempfile.mkdtemp(prefix="macho-fixture-")
    root = build.sysroot()
    for n, t in SOURCES.items():
        with open(os.path.join(top, n), "w") as f:
            f.write(t)
    good = total = 0
    for arch in build.ARCHS:
        for opt in ("-O0", "-O2"):
            d = os.path.join(top, f"{arch}{opt}")
            os.makedirs(d)
            for n in SOURCES:
                if n.endswith(".cpp"):
                    run(["clang++", f"--target={arch}-apple-macos13", *build.UNDEF, *build.includes(arch, root),
                         opt, "-g", "-std=c++17",
                         "-fvisibility-inlines-hidden", "-c", os.path.join(top, n), "-o", n[:-4] + ".o"], d)
            for lib, members in (("liba.a", ["a1.o", "a2.o", "a3.o"]), ("libb.a", ["b1.o", "b2.o", "b3.o"])):
                run([str(B / "llvm-ar"), "rcs", lib, *members], d)
                os.makedirs(os.path.join(d, "folded"), exist_ok=True)
                tool = os.path.join(d, "folded", "llvm-libtool-darwin")
                if not os.path.exists(tool):
                    os.symlink(os.path.abspath(a.turbolib), tool)
                r = run([tool, "-static", "-o", os.path.join("folded", lib), "--fold=identical", "--stats",
                         *members], d)
                stats = [l for l in r.stderr.splitlines() if "folded" in l]
                v = subprocess.run([a.turbolib, "verify", lib, os.path.join("folded", lib)], cwd=d,
                                   capture_output=True, text=True)
                ok_v = "equivalent: no failures" in v.stdout + v.stderr
                total += 1
                good += ok_v
                print(f"{arch} {opt} {lib}: verify {'passes' if ok_v else 'FAILS'}; {stats[0].split(': ', 2)[-1] if stats else ''}")
                if not ok_v:
                    print("   " + (v.stdout + v.stderr)[-1500:].replace("\n", "\n   "))
            for order in (["liba.a", "libb.a"], ["libb.a", "liba.a"]):
                imgs = []
                traces = []
                for which in ("", "folded/"):
                    out = os.path.join(d, f"prog-{which.strip('/') or 'orig'}-{order[0]}")
                    r = subprocess.run(build.NICE + [str(B / "ld64.lld"), "-arch", arch, "-platform_version", "macos",
                                                     "13.0", "13.0", "-undefined", "dynamic_lookup",
                                                     "-no_fixup_chains", "-o", out, "main.o",
                                                     *[which + l for l in order], "-t"],
                                       cwd=d, capture_output=True, text=True)
                    if r.returncode != 0:
                        print(f"  link failed: {r.stderr[:800]}")
                    imgs.append(out)
                    traces.append([l.strip() for l in r.stdout.splitlines() if "(" in l])
                strip = lambda ts: [t.split("/")[-1] for t in ts if "__turbolib." not in t]
                same_members = strip(traces[0]) == strip(traces[1])
                synth = sum(1 for t in traces[1] if "__turbolib." in t)
                for img in imgs:
                    relink.dsym(img)
                c = subprocess.run([a.turbolib, "compare-images", "--examples=3", imgs[0], imgs[1]],
                                   capture_output=True, text=True)
                init = re.search(r"(\d+)  initializers", c.stdout)
                inits = int(init.group(1)) if init else 0
                ok = same_members and c.returncode == 0 and inits >= 2
                total += 1
                good += ok
                print(f"{arch} {opt} {' '.join(order)}: {'equivalent' if ok else 'DIFFERENT'}: members "
                      f"{'same order' if same_members else strip(traces[0]) + strip(traces[1])}, {synth} synthesized; "
                      f"{inits} initializers")
                if c.returncode != 0:
                    print("   " + "\n   ".join(l for l in c.stdout.splitlines() if not re.match(r"\s+\d+  ", l)))
                    print("   " + c.stderr.strip()[:300])
    print(f"{good} of {total} checks pass")
    if a.keep:
        print(top)
    else:
        shutil.rmtree(top, ignore_errors=True)
    sys.exit(0 if good == total else 1)


if __name__ == "__main__":
    main()
