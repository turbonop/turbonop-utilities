#!/usr/bin/env python3
"""Runs llvm-ar, llvm-ranlib, and llvm-libtool-darwin command lines on Mach-O
objects with the real tools and with turbolib in their place, and compares
what each leaves (every file in the directory, byte for byte) and what each
prints and returns. The ELF counterpart is ../elf-corpus/arcompare.py, whose
helpers this uses.

    darcompare.py --turbolib PATH [--llvm-bin DIR] [-v] [scenario...]

The fixture's objects are built here by clang (--target=arm64-apple-macos13
and x86_64-apple-macos13) from sources this script writes, plus a universal
file and an archive made by the real llvm-libtool-darwin.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "elf-corpus"))
import arcompare  # noqa: E402

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
inline int inl(int x) { return x * 2; }
template <class T> struct S { static int v; };
template <class T> int S<T>::v = 5;
int use(int x) { return inl(x) + S<int>::v; }
struct G { G(); };
G g;
""",
    "c_long_name_object_file.c": "int a_long_named_function(void) { return 7; }\n",
    "empty.c": "",
    "abs.s": ".globl _absym\n_absym = 42\n.subsections_via_symbols\n",
}

T = "llvm-libtool-darwin"
SCENARIOS = [
    # llvm-ar and llvm-ranlib on Mach-O members: the Darwin dialect.
    ("ar-rcs", [["llvm-ar", "rcs", "x.a", "a.o", "b.o", "c_long_name_object_file.o", "notes.txt"]]),
    ("ar-qc-ranlib", [["llvm-ar", "qc", "x.a", "a.o", "b.o", "x86/a.o"], ["llvm-ranlib", "x.a"]]),
    ("ar-dup-names", [["llvm-ar", "rcs", "x.a", "a.o", "sub/a.o", "b.o"], ["llvm-ar", "qc", "x.a", "a.o"]]),
    ("ar-U", [["llvm-ar", "rcsU", "x.a", "a.o", "b.o", "sub/a.o"]], {"udate"}),
    ("ar-empty-objects", [["llvm-ar", "rcs", "x.a", "empty.o", "abs.o"]]),
    ("ar-format-darwin-elfless", [["llvm-ar", "--format=darwin", "rcs", "x.a", "notes.txt"]]),
    ("ar-tv-x", [["llvm-ar", "rcs", "x.a", "a.o", "b.o"], ["llvm-ar", "t", "x.a"], ["llvm-ar", "d", "x.a", "a.o"],
                 ["llvm-ar", "x", "x.a"]]),
    ("ar-replace", [["llvm-ar", "rcs", "x.a", "a.o", "b.o"], ["llvm-ar", "r", "x.a", "sub/a.o"]]),
    ("ar-from-libtool", [["llvm-ar", "rcs", "x.a", "lt.a", "a.o"], ["llvm-ar", "s", "lt.a"]]),
    # llvm-libtool-darwin.
    ("lt-basic", [[T, "-static", "-o", "o.a", "a.o", "b.o", "c_long_name_object_file.o"]]),
    ("lt-dash-dash", [[T, "--static", "-o", "o.a", "a.o"]]),
    ("lt-no-static", [[T, "-o", "o.a", "a.o"]]),
    ("lt-no-output", [[T, "-static", "a.o"]]),
    ("lt-o-missing", [[T, "-static", "-o"]]),
    ("lt-no-inputs", [[T, "-static", "-o", "o.a"]]),
    ("lt-D-U", [[T, "-static", "-D", "-U", "-o", "o.a", "a.o"]]),
    ("lt-unknown", [[T, "-static", "-o", "o.a", "-bogus", "a.o"]]),
    ("lt-missing", [[T, "-static", "-o", "o.a", "nope.o"]]),
    ("lt-text", [[T, "-static", "-o", "o.a", "notes.txt"]]),
    ("lt-empty", [[T, "-static", "-o", "o.a", "empty.o", "abs.o"]]),
    ("lt-empty-quiet", [[T, "-static", "-no_warning_for_no_symbols", "-o", "o.a", "empty.o"]]),
    ("lt-empty-werror", [[T, "-static", "-warnings_as_errors", "-o", "o.a", "empty.o"]]),
    ("lt-dups", [[T, "-static", "-o", "o.a", "a.o", "sub/a.o", "b.o", "x86/a.o"]]),
    ("lt-dups-werror", [[T, "-static", "-warnings_as_errors", "-o", "o.a", "a.o", "sub/a.o"]]),
    ("lt-universal", [[T, "-static", "-o", "o.a", "a.o", "x86/a.o", "b.o", "x86/b.o"]]),
    ("lt-universal-input", [[T, "-static", "-o", "o.a", "fat.o", "c_long_name_object_file.o"]]),
    ("lt-universal-archive", [[T, "-static", "-o", "o.a", "fatlib.a"]]),
    ("lt-arch-only", [[T, "-static", "-arch_only", "x86_64", "-o", "o.a", "a.o", "x86/a.o", "x86/b.o"]]),
    ("lt-arch-only-none", [[T, "-static", "-arch_only", "x86_64h", "-o", "o.a", "a.o"]]),
    ("lt-arch-only-bad", [[T, "-static", "-arch_only", "foo", "-o", "o.a", "a.o"]]),
    ("lt-flatten", [[T, "-static", "-o", "o.a", "lt.a", "c_long_name_object_file.o"]]),
    ("lt-l", [[T, "-static", "-o", "o.a", "-Llib", "-lfoo", "-L", "sub", "-la.o"]]),
    ("lt-l-missing", [[T, "-static", "-o", "o.a", "-lnope"]]),
    ("lt-filelist", [[T, "-static", "-o", "o.a", "-filelist", "list.txt", "b.o"]]),
    ("lt-filelist-dir", [[T, "-static", "-o", "o.a", "-filelist", "names.txt,sub"]]),
    ("lt-filelist-missing", [[T, "-static", "-o", "o.a", "-filelist", "nope.txt"]]),
    ("lt-filelist-empty", [[T, "-static", "-o", "o.a", "-filelist", "notes0.txt"]]),
    ("lt-filelist-blank", [[T, "-static", "-o", "o.a", "-filelist", "blank.txt"]]),
    ("lt-dependency-info", [[T, "-static", "-o", "o.a", "-Llib", "-Lsub", "-la.o", "-dependency_info", "dep.dat", "b.o"]]),
    ("lt-U", [[T, "-static", "-U", "-o", "o.a", "a.o", "b.o", "lt.a"]], {"udate"}),
    ("lt-replace", [[T, "-static", "-o", "o.a", "a.o"], [T, "-static", "-o", "o.a", "b.o"]]),
]


def build_fixture(dirname, llvm_bin):
    for d in ("sub", "lib", "x86"):
        os.makedirs(os.path.join(dirname, d))
    for name, text in SOURCES.items():
        with open(os.path.join(dirname, name), "w") as f:
            f.write(text)

    def cc(args):
        subprocess.run(args, cwd=dirname, check=True)

    arm = ["clang", "--target=arm64-apple-macos13", "-c"]
    x86 = ["clang", "--target=x86_64-apple-macos13", "-c"]
    cc(arm + ["-O0", "a.c", "-o", "a.o"])
    cc(arm + ["-O0", "-fcommon", "a.c", "-o", "sub/a.o"])
    cc(arm + ["-O0", "-g", "-x", "c++", "b.cpp", "-o", "b.o"])
    cc(x86 + ["-O0", "a.c", "-o", "x86/a.o"])
    cc(x86 + ["-O1", "-x", "c++", "b.cpp", "-o", "x86/b.o"])
    cc(arm + ["c_long_name_object_file.c", "-o", "c_long_name_object_file.o"])
    cc(arm + ["empty.c", "-o", "empty.o"])
    cc(arm + ["abs.s", "-o", "abs.o"])
    cc(["cp", "a.o", "lib/libfoo.a"])  # (named as a library; it's an object)
    lt = os.path.join(llvm_bin, T)
    cc([lt, "-static", "-o", "lt.a", "a.o", "b.o"])
    cc([os.path.join(llvm_bin, "llvm-lipo"), "-create", "a.o", "x86/a.o", "-output", "fat.o"])
    cc([lt, "-static", "-o", "fatlib.a", "a.o", "x86/a.o", "b.o"])
    with open(os.path.join(dirname, "notes.txt"), "w") as f:
        f.write("not an object\n")
    open(os.path.join(dirname, "notes0.txt"), "w").close()
    with open(os.path.join(dirname, "list.txt"), "w") as f:
        f.write("a.o\nc_long_name_object_file.o\n")
    with open(os.path.join(dirname, "names.txt"), "w") as f:
        f.write("a.o\n")
    with open(os.path.join(dirname, "blank.txt"), "w") as f:
        f.write("a.o\n\nb.o\n")
    for root, _, files in os.walk(dirname):
        for n in files:
            os.utime(os.path.join(root, n), (1_700_000_000, 1_700_000_000))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--turbolib", required=True)
    ap.add_argument("--llvm-bin", default=os.path.expanduser("~/tlcorpus/build-gcc/bin"))
    ap.add_argument("-v", action="store_true")
    ap.add_argument("only", nargs="*")
    args = ap.parse_args()
    top = tempfile.mkdtemp(prefix="darcompare-")
    fixture = os.path.join(top, "fixture")
    os.makedirs(fixture)
    build_fixture(fixture, args.llvm_bin)
    real_bin = os.path.join(top, "real-bin")
    ours_bin = os.path.join(top, "ours-bin")
    os.makedirs(real_bin)
    os.makedirs(ours_bin)
    for n in ("llvm-ar", "llvm-ranlib", T):
        os.symlink(os.path.join(args.llvm_bin, n), os.path.join(real_bin, n))
        os.symlink(os.path.abspath(args.turbolib), os.path.join(ours_bin, n))
    failed = 0
    total = 0
    for s in SCENARIOS:
        name, commands = s[0], s[1]
        flags = s[2] if len(s) > 2 else set()
        if args.only and name not in args.only:
            continue
        total += 1
        real = arcompare.run_scenario(fixture, os.path.join(top, name + "-real"), real_bin, commands)
        ours = arcompare.run_scenario(fixture, os.path.join(top, name + "-ours"), ours_bin, commands)
        problems = []
        for i, (r, o) in enumerate(zip(real[0], ours[0])):
            if r[0] != o[0]:
                problems.append(f"command {i}: exit {r[0]} vs {o[0]}")
            if r[1] != o[1]:
                problems.append(f"command {i}: stdout {r[1][:300]!r} vs {o[1][:300]!r}")
            if r[2] != o[2]:
                problems.append(f"command {i}: stderr {r[2][:300]!r} vs {o[2][:300]!r}")
        for f in sorted(set(real[1]) | set(ours[1])):
            a, b = real[1].get(f), ours[1].get(f)
            if a is None or b is None:
                problems.append(f"{f}: only {'ours' if a is None else 'real'}")
                continue
            if "udate" in flags:
                a, b = arcompare.mask_dates(a), arcompare.mask_dates(b)
            if a != b:
                at = next((k for k in range(min(len(a), len(b))) if a[k] != b[k]), min(len(a), len(b)))
                problems.append(f"{f}: differs at {at} ({len(a)} vs {len(b)} bytes): "
                                f"{a[max(0, at - 20):at + 20]!r} vs {b[max(0, at - 20):at + 20]!r}")
        if problems:
            failed += 1
            print(f"FAIL {name}")
            for p in problems:
                print("    " + p)
        elif args.v:
            print(f"ok   {name}")
    print(f"{total - failed} of {total} scenarios the same")
    shutil.rmtree(top, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
