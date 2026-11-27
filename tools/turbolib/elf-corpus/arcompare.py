#!/usr/bin/env python3
"""Runs ar command lines with a real archiver and with turbolib in its place,
and compares what each leaves: every file in the directory, byte for byte,
and what each prints and returns.

    arcompare.py --turbolib PATH [--llvm-bin DIR] [-v]

Each scenario runs in two fresh copies of a fixture directory (objects built
here, by gcc and g++, from sources this script writes): one with the real
tool (GNU ar and ranlib from PATH; llvm-ar and llvm-ranlib from --llvm-bin),
one with turbolib linked under the same name, first on PATH. Both are run as
the bare name, so their messages name the program alike.

Dates: a scenario with U writes the time into the symbol table (and, for
llvm-ar, the files' times into members): those fields are compared as
"some number" when marked so.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile

SOURCES = {
    "a.c": """
int common_var;
int global_var = 1;
static int local_var = 2;
__attribute__((weak)) int weak_fn(void) { return local_var; }
__attribute__((visibility("hidden"))) int hidden_fn(void) { return 3; }
extern int undef_fn(void);
int calls(void) { return undef_fn() + weak_fn(); }
""",
    "b.cpp": """
#include <string>
#include <vector>
inline int inl(int x) { return x * 2; }
template <class T> struct S { static int v; };
template <class T> int S<T>::v = 5;
std::string use(int x) { std::vector<int> v(x); return std::to_string(inl(x) + S<int>::v + (int)v.size()); }
""",
    "c_long_name_object_file.c": "int a_long_named_function(void) { return 7; }\n",
    "d.cpp": """
#include <map>
#include <string>
std::map<std::string, int> m;
int get(const std::string &s) { return m[s]; }
""",
    "empty.c": "",
    "abs.s": ".globl absym\n.set absym, 42\n",
}

# (name, tool, commands, flags) where flags may hold "udate" (dates vary).
# Each command is a list of words; the first is the program.
SCENARIOS = [
    ("rcs", "ar", [["ar", "rcs", "x.a", "a.o", "b.o", "c_long_name_object_file.o", "notes.txt", "odd.txt"]]),
    ("qc-ranlib", "ar", [["ar", "qc", "x.a", "a.o", "b.o", "d.o"], ["ranlib", "x.a"]]),
    ("qcS", "ar", [["ar", "qcS", "x.a", "a.o", "b.o"]]),
    ("q-twice", "ar", [["ar", "q", "x.a", "a.o"], ["ar", "q", "x.a", "a.o", "b.o"]]),
    ("cr-creating", "ar", [["ar", "r", "x.a", "a.o"]]),
    ("r-replace", "ar", [["ar", "rc", "x.a", "a.o", "b.o", "d.o"], ["cp", "b2.o", "b.o"], ["ar", "r", "x.a", "b.o"]]),
    ("r-dup-names", "ar", [["ar", "rc", "x.a", "c_long_name_object_file.o", "sub/c_long_name_object_file.o", "sub/a.o", "a.o"]]),
    ("rP", "ar", [["ar", "rcP", "x.a", "sub/a.o", "a.o", "sub/c_long_name_object_file.o"]]),
    ("ra", "ar", [["ar", "rc", "x.a", "a.o", "b.o"], ["ar", "rca", "a.o", "x.a", "d.o"]]),
    ("rb", "ar", [["ar", "rc", "x.a", "a.o", "b.o"], ["ar", "rcb", "b.o", "x.a", "d.o"]]),
    ("d", "ar", [["ar", "rc", "x.a", "a.o", "b.o", "d.o"], ["ar", "dv", "x.a", "b.o", "zz.o"]]),
    ("dN", "ar", [["ar", "qc", "x.a", "a.o", "b.o", "a.o"], ["ar", "dN", "2", "x.a", "a.o"]]),
    ("m", "ar", [["ar", "rc", "x.a", "a.o", "b.o", "d.o"], ["ar", "mv", "x.a", "a.o"]]),
    ("ma", "ar", [["ar", "rc", "x.a", "a.o", "b.o", "d.o"], ["ar", "ma", "b.o", "x.a", "d.o"]]),
    ("t", "ar", [["ar", "rc", "x.a", "a.o", "b.o", "c_long_name_object_file.o"], ["ar", "t", "x.a"]]),
    ("tv", "ar", [["ar", "rc", "x.a", "a.o", "b.o"], ["ar", "tv", "x.a"]]),
    ("tO", "ar", [["ar", "rc", "x.a", "a.o", "b.o", "odd.txt"], ["ar", "tO", "x.a"]]),
    ("t-names", "ar", [["ar", "rc", "x.a", "a.o", "b.o", "d.o"], ["ar", "t", "x.a", "d.o", "a.o", "zz.o"]]),
    ("p", "ar", [["ar", "rc", "x.a", "notes.txt", "odd.txt"], ["ar", "p", "x.a"], ["ar", "pv", "x.a", "odd.txt"]]),
    ("x", "ar", [["ar", "rc", "x.a", "a.o", "notes.txt"], ["rm", "a.o", "notes.txt"], ["ar", "xv", "x.a"]]),
    ("s", "ar", [["ar", "rcS", "x.a", "a.o", "b.o"], ["ar", "s", "x.a"]]),
    ("thin", "ar", [["ar", "rcsT", "lib/x.a", "a.o", "sub/a.o", "c_long_name_object_file.o"]]),
    ("thin-twice", "ar", [["ar", "qcT", "x.a", "a.o", "a.o"]]),
    ("thin-nested", "ar", [["ar", "rc", "lib/y.a", "a.o", "b.o"], ["ar", "rcT", "x.a", "lib/y.a", "d.o"]]),
    ("thin-t", "ar", [["ar", "rcT", "lib/x.a", "a.o", "sub/a.o"], ["ar", "t", "lib/x.a"]]),
    ("archive-member", "ar", [["ar", "rc", "y.a", "a.o"], ["ar", "rc", "x.a", "y.a", "b.o"]]),
    ("no-objects", "ar", [["ar", "rcs", "x.a", "notes.txt", "odd.txt"]]),
    ("empty-object", "ar", [["ar", "rcs", "x.a", "empty.o"]]),
    ("abs-common", "ar", [["ar", "rcs", "x.a", "sub/a.o", "abs.o"]]),
    ("dash", "ar", [["ar", "-rcs", "x.a", "a.o", "b.o"]]),
    ("libdeps", "ar", [["ar", "rc", "--record-libdeps", "-lfoo -lbar", "x.a", "a.o", "b.o"]]),
    ("mri", "ar", [["sh", "-c", "printf 'create x.a\\naddmod a.o b.o\\naddmod d.o\\nsave\\nend\\n' | ar -M"]]),
    ("mri-addlib", "ar", [["ar", "rc", "y.a", "a.o", "b.o"], ["sh", "-c", "printf 'create x.a\\naddlib y.a\\naddmod d.o\\ndelete a.o\\nsave\\nend\\n' | ar -M"]]),
    ("ranlib-U", "ar", [["ar", "rcU", "x.a", "a.o"], ["ranlib", "-U", "x.a"]], {"udate"}),
    ("rcsU", "ar", [["ar", "rcsU", "x.a", "a.o", "b.o"]], {"udate"}),
    ("missing", "ar", [["ar", "rcs", "x.a", "a.o", "missing.o"]]),
    # llvm-ar.
    ("l-rcs", "llvm-ar", [["llvm-ar", "rcs", "x.a", "a.o", "b.o", "c_long_name_object_file.o", "notes.txt", "odd.txt"]]),
    ("l-qc-ranlib", "llvm-ar", [["llvm-ar", "qc", "x.a", "a.o", "b.o", "d.o"], ["llvm-ranlib", "x.a"]]),
    ("l-qcS-ranlib", "llvm-ar", [["llvm-ar", "qcS", "x.a", "a.o", "b.o"], ["llvm-ranlib", "x.a"]]),
    ("l-dup", "llvm-ar", [["llvm-ar", "rc", "x.a", "c_long_name_object_file.o", "sub/c_long_name_object_file.o", "sub/a.o", "a.o"]]),
    ("l-r-replace", "llvm-ar", [["llvm-ar", "rc", "x.a", "a.o", "b.o", "d.o"], ["cp", "b2.o", "b.o"], ["llvm-ar", "r", "x.a", "b.o"]]),
    ("l-ra", "llvm-ar", [["llvm-ar", "rc", "x.a", "a.o", "b.o"], ["llvm-ar", "rca", "a.o", "x.a", "d.o"]]),
    ("l-d", "llvm-ar", [["llvm-ar", "rc", "x.a", "a.o", "b.o", "d.o"], ["llvm-ar", "d", "x.a", "b.o"]]),
    ("l-m", "llvm-ar", [["llvm-ar", "rc", "x.a", "a.o", "b.o", "d.o"], ["llvm-ar", "m", "x.a", "a.o"]]),
    ("l-tv", "llvm-ar", [["llvm-ar", "rc", "x.a", "a.o", "b.o"], ["llvm-ar", "tv", "x.a"]]),
    ("l-tO", "llvm-ar", [["llvm-ar", "rc", "x.a", "a.o", "odd.txt"], ["llvm-ar", "tO", "x.a"]]),
    ("l-x", "llvm-ar", [["llvm-ar", "rc", "x.a", "a.o", "notes.txt"], ["rm", "a.o", "notes.txt"], ["llvm-ar", "xv", "x.a"]]),
    ("l-p", "llvm-ar", [["llvm-ar", "rc", "x.a", "notes.txt", "odd.txt"], ["llvm-ar", "pv", "x.a"]]),
    ("l-thin", "llvm-ar", [["llvm-ar", "rcs", "--thin", "lib/x.a", "a.o", "sub/a.o", "c_long_name_object_file.o"]]),
    ("l-thin-twice", "llvm-ar", [["llvm-ar", "qcT", "x.a", "a.o", "a.o"]]),
    ("l-thin-t", "llvm-ar", [["llvm-ar", "rc", "--thin", "lib/x.a", "a.o", "sub/a.o"], ["llvm-ar", "t", "lib/x.a"]]),
    ("l-L", "llvm-ar", [["llvm-ar", "rc", "y.a", "a.o", "b.o"], ["llvm-ar", "qcL", "x.a", "y.a", "d.o"]]),
    ("l-no-objects", "llvm-ar", [["llvm-ar", "rcs", "x.a", "notes.txt", "odd.txt"]]),
    ("l-empty-object", "llvm-ar", [["llvm-ar", "rcs", "x.a", "empty.o"]]),
    ("l-bsd", "llvm-ar", [["llvm-ar", "rcs", "--format=bsd", "x.a", "a.o", "b.o", "c_long_name_object_file.o", "odd.txt"]]),
    ("l-darwin", "llvm-ar", [["llvm-ar", "rcs", "--format=darwin", "x.a", "a.o", "b.o", "odd.txt", "a.o"]]),
    ("l-mri", "llvm-ar", [["sh", "-c", "printf 'create x.a\\naddmod a.o\\naddmod b.o\\nsave\\nend\\n' | llvm-ar -M"]]),
    ("l-sym64", "llvm-ar", [["env", "SYM64_THRESHOLD=100", "llvm-ar", "rcs", "x.a", "a.o", "b.o"]]),
    ("l-ranlib-has-table", "llvm-ar", [["llvm-ar", "rcsU", "x.a", "a.o"], ["llvm-ranlib", "x.a"]], {"udate"}),
    ("l-missing", "llvm-ar", [["llvm-ar", "rcs", "x.a", "a.o", "missing.o"]]),
]


def build_fixture(dirname):
    os.makedirs(os.path.join(dirname, "sub"))
    os.makedirs(os.path.join(dirname, "lib"))
    for name, text in SOURCES.items():
        with open(os.path.join(dirname, name), "w") as f:
            f.write(text)
    def cc(args):
        subprocess.run(args, cwd=dirname, check=True)
    cc(["gcc", "-c", "-O0", "a.c", "-o", "a.o"])
    cc(["gcc", "-c", "-O0", "-fcommon", "a.c", "-o", "sub/a.o"])
    cc(["g++", "-c", "-O0", "-g", "-ffunction-sections", "b.cpp", "-o", "b.o"])
    cc(["g++", "-c", "-O1", "b.cpp", "-o", "b2.o"])
    cc(["g++", "-c", "-O0", "d.cpp", "-o", "d.o"])
    cc(["gcc", "-c", "c_long_name_object_file.c", "-o", "c_long_name_object_file.o"])
    cc(["cp", "c_long_name_object_file.o", "sub/"])
    cc(["gcc", "-c", "empty.c", "-o", "empty.o"])
    cc(["gcc", "-c", "abs.s", "-o", "abs.o"])
    with open(os.path.join(dirname, "notes.txt"), "w") as f:
        f.write("not an object\n")
    with open(os.path.join(dirname, "odd.txt"), "w") as f:
        f.write("odd")
    # The same times everywhere, so U compares.
    for root, _, files in os.walk(dirname):
        for n in files:
            os.utime(os.path.join(root, n), (1_700_000_000, 1_700_000_000))


def snapshot(dirname):
    out = {}
    for root, _, files in os.walk(dirname):
        for n in files:
            p = os.path.join(root, n)
            with open(p, "rb") as f:
                out[os.path.relpath(p, dirname)] = f.read()
    return out


def mask_dates(data):
    """An archive with its headers' date fields blanked (for U)."""
    if not (data.startswith(b"!<arch>\n") or data.startswith(b"!<thin>\n")):
        return data
    b = bytearray(data)
    at = 8
    thin = data.startswith(b"!<thin>\n")
    while at + 60 <= len(b):
        name = bytes(b[at:at + 16]).strip()
        try:
            size = int(b[at + 48:at + 58])
        except ValueError:
            break
        b[at + 16:at + 28] = b" " * 12
        table = name in (b"/", b"//", b"/SYM64/") or name.startswith(b"#1/")
        at += 60 + (size + (size & 1) if (not thin or table) else 0)
    return bytes(b)


def run_scenario(fixture, work, tools_dir, commands):
    shutil.copytree(fixture, work)
    env = dict(os.environ)
    env["PATH"] = tools_dir + os.pathsep + env["PATH"]
    env.pop("SOURCE_DATE_EPOCH", None)
    results = []
    for cmd in commands:
        p = subprocess.run(cmd, cwd=work, env=env, capture_output=True)
        results.append((p.returncode, p.stdout, p.stderr))
    return results, snapshot(work)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--turbolib", required=True)
    ap.add_argument("--llvm-bin", default=os.path.expanduser("~/tlcorpus/build-gcc/bin"))
    ap.add_argument("-v", action="store_true")
    ap.add_argument("only", nargs="*")
    args = ap.parse_args()
    top = tempfile.mkdtemp(prefix="arcompare-")
    fixture = os.path.join(top, "fixture")
    build_fixture(fixture)
    real_bin = os.path.join(top, "real-bin")
    ours_bin = os.path.join(top, "ours-bin")
    os.makedirs(real_bin)
    os.makedirs(ours_bin)
    for n in ("llvm-ar", "llvm-ranlib"):
        # (llvm-ranlib is llvm-ar by another name, if the build has none yet.)
        source = os.path.join(args.llvm_bin, n)
        if not os.path.exists(source):
            source = os.path.join(args.llvm_bin, "llvm-ar")
        os.symlink(source, os.path.join(real_bin, n))
    for n in ("ar", "ranlib", "llvm-ar", "llvm-ranlib"):
        os.symlink(os.path.abspath(args.turbolib), os.path.join(ours_bin, n))
    failed = 0
    for s in SCENARIOS:
        name, _tool, commands = s[0], s[1], s[2]
        flags = s[3] if len(s) > 3 else set()
        if args.only and name not in args.only:
            continue
        real = run_scenario(fixture, os.path.join(top, name + "-real"), real_bin, commands)
        ours = run_scenario(fixture, os.path.join(top, name + "-ours"), ours_bin, commands)
        problems = []
        for i, (r, o) in enumerate(zip(real[0], ours[0])):
            if r[0] != o[0]:
                problems.append(f"command {i}: exit {r[0]} vs {o[0]}")
            if r[1] != o[1]:
                problems.append(f"command {i}: stdout {r[1][:300]!r} vs {o[1][:300]!r}")
            if r[2] != o[2]:
                problems.append(f"command {i}: stderr {r[2][:300]!r} vs {o[2][:300]!r}")
        files = sorted(set(real[1]) | set(ours[1]))
        for f in files:
            a, b = real[1].get(f), ours[1].get(f)
            if a is None or b is None:
                problems.append(f"{f}: only {'ours' if a is None else 'real'}")
                continue
            if "udate" in flags:
                a, b = mask_dates(a), mask_dates(b)
            if a != b:
                at = next((k for k in range(min(len(a), len(b))) if a[k] != b[k]), min(len(a), len(b)))
                problems.append(f"{f}: differs at {at} ({len(a)} vs {len(b)} bytes): {a[max(0, at - 20):at + 20]!r} vs {b[max(0, at - 20):at + 20]!r}")
        if problems:
            failed += 1
            print(f"FAIL {name}")
            for p in problems:
                print("    " + p)
        elif args.v:
            print(f"ok   {name}")
    total = sum(1 for s in SCENARIOS if not args.only or s[0] in args.only)
    print(f"{total - failed} of {total} scenarios the same")
    shutil.rmtree(top, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
