#!/usr/bin/env python3
"""A Mach-O corpus for turbolib, built without a Mac or an SDK.

Each object of the libraries `llvm-nm` and `llvm-readobj` link (and those
two tools' own objects) is compiled again from the ELF corpus's clang build
(`elf-corpus/configure.sh clang`: its `compile_commands.json`, generated
headers, and `build.ninja`), for `arm64-apple-macos13` and
`x86_64-apple-macos13`, with `-g`, and archived the ways a Mac build does.

There is no macOS SDK here, so the C and C++ headers are Linux's: glibc and
libstdc++ (the x86-64 ones installed, and for arm64 Ubuntu's
`*-arm64-cross` packages, which `sysroot` fetches with `apt-get download`
and unpacks, without root). The compiler's Apple macros (`__APPLE__`,
`__MACH__`, and the nullability keywords `__nonnull`, `__nullable`,
`__null_unspecified`) are undefined so those headers and LLVM take their
Linux paths, and x86-64 glibc's `bits/floatn.h` is overlaid to say there is
no `_Float128` (Darwin's clang has none). The code differs from a Mac
build's, but the objects are what clang writes for these targets:
`MH_SUBSECTIONS_VIA_SYMBOLS`, weak definitions (private extern under
`-fvisibility-inlines-hidden`), `__compact_unwind`, `__DWARF`, and so on.

    build.py [--jobs N] [--only LIB,...]

CORPUS (default ~/tlcorpus) holds `build-clang` and receives `macho/`:

    macho/sysroot/                  the arm64 headers, the x86-64 overlay
    macho/obj/<arch>/<path>.o       each object, at the build's own path
    macho/lib/llvm-ar/<arch>/       `llvm-ar qc` then `llvm-ranlib` (CMake's
                                    rules with CMAKE_AR=llvm-ar)
    macho/lib/libtool/<arch>/       `llvm-libtool-darwin -static`
    macho/lib/libtool/universal/    the same of both archs' objects: a
                                    universal file of two archives
    macho/members/<lib>.txt         each library's objects, in order
    macho/failed.txt                objects that didn't compile (left out)

The tools (`llvm-ar`, `llvm-ranlib`, `llvm-libtool-darwin`) are the gcc
build's (`build-gcc/bin`, LLVM at the corpus commit); LLVM_BIN overrides.
Everything runs at low priority (`nice -n 19`) on every core.
"""

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

CORPUS = Path(os.environ.get("CORPUS", Path.home() / "tlcorpus"))
BUILD = CORPUS / "build-clang"
OUT = CORPUS / "macho"
LLVM_BIN = Path(os.environ.get("LLVM_BIN", CORPUS / "build-gcc" / "bin"))
CLANG = os.environ.get("MACHO_CXX", "/usr/bin/clang++")
CLANG_C = os.environ.get("MACHO_CC", "/usr/bin/clang")
RESOURCE = os.environ.get("CLANG_RESOURCE", "/usr/lib/llvm-21/lib/clang/21/include")
ARCHS = ["arm64", "x86_64"]
TOOLS = ["bin/llvm-nm", "bin/llvm-readobj"]
NICE = ["nice", "-n", "19"]


def sysroot():
    root = OUT / "sysroot"
    debs = root / "debs"
    debs.mkdir(parents=True, exist_ok=True)
    if not (debs / "done").exists():
        pkgs = ["libc6-dev-arm64-cross", "linux-libc-dev-arm64-cross", "libstdc++-15-dev-arm64-cross"]
        subprocess.run(["apt-get", "download", *pkgs], cwd=debs, check=True, capture_output=True)
        for d in sorted(debs.glob("*.deb")):
            subprocess.run(["dpkg", "-x", str(d), str(root)], check=True)
        (debs / "done").touch()
    ovr = root / "x86ovr" / "bits"
    ovr.mkdir(parents=True, exist_ok=True)
    text = Path("/usr/include/x86_64-linux-gnu/bits/floatn.h").read_text()
    (ovr / "floatn.h").write_text(text.replace("# define __HAVE_FLOAT128 1", "# define __HAVE_FLOAT128 0"))
    return root


def includes(arch, root):
    if arch == "arm64":
        g = root / "usr/aarch64-linux-gnu/include"
        return ["-nostdinc", "-nostdinc++", "-isystem", RESOURCE, "-isystem", f"{g}/c++/15",
                "-isystem", f"{g}/c++/15/aarch64-linux-gnu", "-isystem", str(g)]
    return ["-nostdinc++", "-isystem", str(root / "x86ovr"), "-isystem", "/usr/include/c++/15",
            "-isystem", "/usr/include/x86_64-linux-gnu/c++/15", "-isystem", "/usr/include/x86_64-linux-gnu"]


UNDEF = ["-U__APPLE__", "-U__MACH__", "-U__nonnull", "-U__nullable", "-U__null_unspecified",
         "-include", "endian.h"]
DROP_ONE = {"-ffunction-sections", "-fdata-sections", "-fno-semantic-interposition", "-fPIC",
            "-fdiagnostics-color", "-Winvalid-pch", "-pedantic"}


def rewrite(entry, arch, root):
    args = shlex.split(entry["command"]) if "command" in entry else list(entry["arguments"])
    cc = args[0]
    out = None
    keep = []
    i = 1
    while i < len(args):
        a = args[i]
        if a == "-o":
            out = args[i + 1]
            i += 2
            continue
        if a == "-Xclang" and i + 1 < len(args) and args[i + 1] in ("-include-pch", "-include", "-fno-pch-timestamp"):
            i += 2 if args[i + 1] == "-fno-pch-timestamp" else 4
            continue
        if a == "-c":
            i += 1
            continue
        if a in DROP_ONE or (a.startswith("-W") and not a.startswith("-Wl")):
            i += 1
            continue
        keep.append(a)
        i += 1
    src = keep.pop()  # the source is last
    compiler = CLANG_C if (cc.endswith("clang") or cc.endswith("gcc")) else CLANG
    obj = OUT / "obj" / arch / out
    cmd = [compiler, f"--target={arch}-apple-macos13", *UNDEF, *includes(arch, root), *keep,
           "-w", "-o", str(obj), "-c", src]
    return out, obj, cmd


def ninja_libs():
    """Each static library's objects, from build.ninja, and the tools'."""
    libs, tools = {}, {}
    text = (BUILD / "build.ninja").read_text()
    for line in text.splitlines():
        m = re.match(r"build (lib/lib[\w.+-]+\.a): CXX_STATIC_LIBRARY_LINKER__\S+ (.*)", line)
        if m:
            parts = m.group(2).split(" || ")[0].split(" | ")[0].split()
            libs[m.group(1)] = [p for p in parts if p.endswith(".o")]
            continue
        m = re.match(r"build (bin/[\w.+-]+): CXX_EXECUTABLE_LINKER__\S+ (.*)", line)
        if m and m.group(1) in TOOLS:
            deps = m.group(2).split(" || ")[0]
            objs, implicit = deps.split(" | ")[0].split(), deps.split(" | ")[1].split() if " | " in deps else []
            tools[m.group(1)] = ([p for p in objs if p.endswith(".o")],
                                 [p for p in implicit + objs if p.startswith("lib/lib") and p.endswith(".a")])
    return libs, tools


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    ap.add_argument("--only", default="")
    a = ap.parse_args()
    root = sysroot()
    libs, tools = ninja_libs()
    wanted = set()
    for _, ls in tools.values():
        wanted.update(ls)
    if a.only:
        wanted = {f"lib/lib{n}.a" for n in a.only.split(",")}
    wanted = sorted(w for w in wanted if w in libs)
    objs = {o for w in wanted for o in libs[w]}
    for o, _ in tools.values():
        objs.update(o)
    cc = {}
    for e in json.load(open(BUILD / "compile_commands.json")):
        out = shlex.split(e["command"])[shlex.split(e["command"]).index("-o") + 1] if "command" in e else None
        if out in objs:
            cc[out] = e
    jobs = []
    for arch in ARCHS:
        for out in sorted(cc):
            o, obj, cmd = rewrite(cc[out], arch, root)
            jobs.append((arch, o, obj, cmd, cc[out]["directory"]))

    def compile_one(job):
        arch, o, obj, cmd, cwd = job
        if obj.exists():
            return None
        obj.parent.mkdir(parents=True, exist_ok=True)
        r = subprocess.run(NICE + cmd, cwd=cwd, capture_output=True, text=True)
        if r.returncode != 0:
            if obj.exists():
                obj.unlink()
            return f"{arch} {o}: {r.stderr.strip().splitlines()[-1] if r.stderr.strip() else r.returncode}"
        return None

    with ThreadPoolExecutor(a.jobs) as ex:
        failed = [f for f in ex.map(compile_one, jobs) if f]
    (OUT / "failed.txt").write_text("".join(f + "\n" for f in failed))
    print(f"{len(jobs)} compiles, {len(failed)} failed", flush=True)

    # Archive each library: llvm-ar qc + llvm-ranlib, and llvm-libtool-darwin.
    (OUT / "members").mkdir(parents=True, exist_ok=True)
    arjobs = []
    for w in wanted:
        name = Path(w).name
        (OUT / "members" / (name + ".txt")).write_text("".join(o + "\n" for o in libs[w]))
        per_arch = {}
        for arch in ARCHS:
            present = [str(OUT / "obj" / arch / o) for o in libs[w] if (OUT / "obj" / arch / o).exists()]
            per_arch[arch] = present
            if not present:
                continue
            for d in ("llvm-ar", "libtool"):
                (OUT / "lib" / d / arch).mkdir(parents=True, exist_ok=True)
            ar = OUT / "lib" / "llvm-ar" / arch / name
            lt = OUT / "lib" / "libtool" / arch / name
            arjobs.append([["rm", "-f", str(ar)], [str(LLVM_BIN / "llvm-ar"), "qc", str(ar), *present],
                           [str(LLVM_BIN / "llvm-ranlib"), str(ar)]])
            arjobs.append([["rm", "-f", str(lt)], [str(LLVM_BIN / "llvm-libtool-darwin"), "-static",
                                                    "-no_warning_for_no_symbols", "-o", str(lt), *present]])
        both = per_arch["arm64"] + per_arch["x86_64"]
        if per_arch["arm64"] and per_arch["x86_64"]:
            (OUT / "lib" / "libtool" / "universal").mkdir(parents=True, exist_ok=True)
            u = OUT / "lib" / "libtool" / "universal" / name
            arjobs.append([["rm", "-f", str(u)], [str(LLVM_BIN / "llvm-libtool-darwin"), "-static",
                                                   "-no_warning_for_no_symbols", "-o", str(u), *both]])

    def archive(cmds):
        for c in cmds:
            r = subprocess.run(NICE + c, capture_output=True, text=True)
            if r.returncode != 0:
                return f"{' '.join(c[:3])}: {r.stderr.strip()[:300]}"
        return None

    with ThreadPoolExecutor(a.jobs) as ex:
        bad = [b for b in ex.map(archive, arjobs) if b]
    for b in bad:
        print(b, file=sys.stderr)
    print(f"{len(wanted)} libraries, {len(arjobs)} archives, {len(bad)} failed")


if __name__ == "__main__":
    main()
