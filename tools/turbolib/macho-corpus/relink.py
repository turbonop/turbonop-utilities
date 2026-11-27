#!/usr/bin/env python3
"""Links llvm-nm and llvm-readobj (arm64 and x86_64 macOS) with ld64.lld
against the Mach-O corpus's libraries and against folded ones, and compares
the two links. Nothing can be run without a Mac, so this compares what the
images hold.

    relink.py --folded DIR --turbolib PATH [--kind llvm-ar|libtool] [--keep]

DIR holds lib/<kind>/<arch>/lib*.a (relib.py's --out, run with
--options=--fold=identical). Each link uses `-undefined dynamic_lookup`
(there is no SDK to link against) and `-t`. Compared:

- **members loaded**: `-t`'s list of archive members, in order, without the
  synthesized ones (`__turbolib.N.o`), which are counted;
- **everything else** by `turbolib compare-images` of the two images, each
  with its dSYM (`dsymutil --verify-dwarf=none`: clang 21's objects fail
  its input verification, and it then writes nothing): functions and data
  by content, symbols, initializers in order, unwind information, imports,
  and the DWARF (see `turbonop_verify::macho_image`).

Runs at low priority, the four configurations in parallel.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build  # noqa: E402

B = build.LLVM_BIN
NICE = build.NICE


def link_inputs(tool, arch, libdir):
    text = (build.BUILD / "build.ninja").read_text()
    line = next(l for l in text.splitlines() if l.startswith(f"build {tool}: CXX_EXECUTABLE_LINKER"))
    words = line.split(" || ")[0].replace(" | ", " ").split()
    objs = [str(build.OUT / "obj" / arch / w) for w in words if w.endswith(".o")]
    libs, seen = [], set()
    for w in words:
        if w.startswith("lib/lib") and w.endswith(".a") and w not in seen:
            seen.add(w)
            libs.append(str(Path(libdir) / arch / Path(w).name))
    return objs, libs


def link(tool, arch, libdir, out):
    objs, libs = link_inputs(tool, arch, libdir)
    r = subprocess.run(NICE + [str(B / "ld64.lld"), "-arch", arch, "-platform_version", "macos", "13.0", "13.0",
                               "-undefined", "dynamic_lookup", "-no_fixup_chains", "-o", out, *objs, *libs, "-t"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"link failed: {r.stderr[:2000]}")
    return [l.strip() for l in r.stdout.splitlines() if l.strip()]


def dsym(image):
    """Makes the image's dSYM (`<image>.dSYM`), as a Mac build would (with the
    classic linker, which Apple's dsymutil uses; this LLVM's defaults to its
    parallel one)."""
    subprocess.run(NICE + [str(B / "dsymutil"), "--linker=classic", "--verify-dwarf=none", image],
                   capture_output=True, check=True)


def compare(tool, arch, kind, folded, tmp, turbolib):
    name = Path(tool).name
    a = os.path.join(tmp, f"{name}-{arch}-orig")
    b = os.path.join(tmp, f"{name}-{arch}-fold")
    ta = link(tool, arch, build.OUT / "lib" / kind, a)
    tb = link(tool, arch, Path(folded) / "lib" / kind, b)
    lines = [f"{name} {arch}:"]
    ma = [t for t in ta if "(" in t]
    mb = [t for t in tb if "(" in t and "__turbolib." not in t]
    synth = sum(1 for t in tb if "__turbolib." in t)
    strip = lambda ts: [re.sub(r"^.*/", "", t) for t in ts]
    same_members = strip(ma) == strip(mb)
    lines.append(f"  members loaded: {len(ma)} original vs {len(mb)}, {'the same, in order' if same_members else 'DIFFERENT'}; "
                 f"{synth} synthesized")
    if not same_members:
        sa, sb = strip(ma), strip(mb)
        k = next((i for i in range(min(len(sa), len(sb))) if sa[i] != sb[i]), min(len(sa), len(sb)))
        lines.append(f"    first difference at {k}: {sa[k] if k < len(sa) else None} vs {sb[k] if k < len(sb) else None}")
    dsym(a)
    dsym(b)
    r = subprocess.run(NICE + [turbolib, "compare-images", "--examples=3", a, b], capture_output=True, text=True)
    lines += ["  " + l for l in r.stdout.splitlines()]
    if r.returncode not in (0, 1):
        lines.append(f"  compare-images failed: {r.stderr.strip()[:500]}")
    return same_members and r.returncode == 0, "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--folded", required=True)
    ap.add_argument("--turbolib", required=True)
    ap.add_argument("--kind", default="llvm-ar")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    tmp = tempfile.mkdtemp(prefix="relink-macho-")
    configs = [(t, arch) for t in build.TOOLS for arch in build.ARCHS]
    with ThreadPoolExecutor(len(configs)) as ex:
        results = list(ex.map(lambda c: compare(c[0], c[1], a.kind, a.folded, tmp, a.turbolib), configs))
    for _, text in results:
        print(text)
    good = sum(1 for ok, _ in results if ok)
    print(f"{good} of {len(results)} links equivalent")
    if a.keep:
        print(f"images in {tmp}")
    else:
        shutil.rmtree(tmp, ignore_errors=True)
    sys.exit(0 if good == len(results) else 1)


if __name__ == "__main__":
    main()
