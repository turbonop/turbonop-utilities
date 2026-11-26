#!/usr/bin/env python3
"""Links llvm-nm and llvm-readobj (arm64 and x86_64 macOS) with ld64.lld
against the Mach-O corpus's libraries and against folded ones, and compares
the two links structurally. Nothing can be run without a Mac, so this
compares what the images hold.

    relink.py --folded DIR [--kind llvm-ar|libtool] [--keep]

DIR holds lib/<kind>/<arch>/lib*.a (relib.py's --out, run with
--options=--fold=identical). Each link uses `-undefined dynamic_lookup`
(there is no SDK to link against) and `-t`. Compared:

- **members loaded**: `-t`'s list of archive members, in order, without the
  synthesized ones (`__turbolib.N.o`), which are counted;
- **symbols**: `llvm-nm -m` of each image (type, external or not, section,
  name; no addresses), as sets: the anchors (`!turbolib.anchor.`) are
  expected only in the folded image;
- **code**: each function's disassembly (`llvm-objdump -d`), with addresses
  and immediates of address computations taken out and branch targets by
  name, the same per function name;
- **initializers**: the names `__mod_init_func`'s entries point to, in
  order (`llvm-objdump --macho --section=__DATA,__mod_init_func` symbolized
  through the image's symbols);
- **sections**: each section's size (reported, not required to match).

Runs at low priority, the four configurations in parallel.
"""

import argparse
import os
import re
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
                               "-undefined", "dynamic_lookup", "-o", out, *objs, *libs, "-t"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"link failed: {r.stderr[:2000]}")
    return [l.strip() for l in r.stdout.splitlines() if l.strip()]


def symbols(image):
    r = subprocess.run(NICE + [str(B / "llvm-nm"), "-m", image], capture_output=True, text=True, check=True)
    out = set()
    for l in r.stdout.splitlines():
        parts = l.split(None, 1)
        if len(parts) == 2 and re.fullmatch(r"[0-9a-f]+", parts[0]):
            out.add(parts[1])
        else:
            out.add(l.strip())
    return out


ADDR = re.compile(r"0x[0-9a-f]+|#-?\d+|\b\d+\b")


def functions(image):
    r = subprocess.run(NICE + [str(B / "llvm-objdump"), "-d", "--no-show-raw-insn", "--no-leading-addr", image],
                       capture_output=True, text=True, check=True)
    funcs, name, body = {}, None, []
    for l in r.stdout.splitlines():
        m = re.match(r"^(?:[0-9a-f]+ )?<(.+)>:$", l)
        if m:
            if name is not None:
                funcs[name] = body
            name, body = m.group(1), []
            continue
        if name is None or not l.strip() or l.startswith("Disassembly"):
            continue
        ins = l.strip()
        targets = re.findall(r"<([^>]+)>", ins)
        ins = re.sub(r"<[^>]+>", "", ins)
        body.append((ADDR.sub("N", ins).strip(), tuple(t.split("+")[0] for t in targets)))
    if name is not None:
        funcs[name] = body
    return funcs


def sections(image):
    r = subprocess.run(NICE + [str(B / "llvm-objdump"), "--section-headers", image], capture_output=True,
                       text=True, check=True)
    out = {}
    for l in r.stdout.splitlines():
        p = l.split()
        if len(p) >= 4 and p[0].isdigit():
            out[p[1]] = int(p[2], 16)
    return out


def init_order(image):
    """The names __mod_init_func's (or __init_offsets') entries point to."""
    r = subprocess.run(NICE + [str(B / "llvm-objdump"), "--macho", "--section-headers", image], capture_output=True,
                       text=True)
    has = "__mod_init_func" in r.stdout or "__init_offsets" in r.stdout
    if not has:
        return []
    sec = "__DATA_CONST,__mod_init_func" if "__DATA_CONST" in r.stdout else "__DATA,__mod_init_func"
    r = subprocess.run(NICE + [str(B / "llvm-objdump"), "--macho", f"--section={sec}", image],
                       capture_output=True, text=True)
    return [l.split()[-1] for l in r.stdout.splitlines() if l.strip() and not l.startswith(("Contents", image))]


def compare(tool, arch, kind, folded, tmp):
    name = Path(tool).name
    a = os.path.join(tmp, f"{name}-{arch}-orig")
    b = os.path.join(tmp, f"{name}-{arch}-fold")
    ta = link(tool, arch, build.OUT / "lib" / kind, a)
    tb = link(tool, arch, Path(folded) / "lib" / kind, b)
    lines = [f"{name} {arch}:"]
    ok = True
    ma = [t for t in ta if "(" in t]
    mb = [t for t in tb if "(" in t and "__turbolib." not in t]
    synth = sum(1 for t in tb if "__turbolib." in t)
    strip = lambda ts: [re.sub(r"^.*/", "", t) for t in ts]
    same_members = strip(ma) == strip(mb)
    ok &= same_members
    lines.append(f"  members loaded: {len(ma)} original vs {len(mb)}, {'the same, in order' if same_members else 'DIFFERENT'}; "
                 f"{synth} synthesized")
    if not same_members:
        sa, sb = strip(ma), strip(mb)
        k = next((i for i in range(min(len(sa), len(sb))) if sa[i] != sb[i]), min(len(sa), len(sb)))
        lines.append(f"    first difference at {k}: {sa[k] if k < len(sa) else None} vs {sb[k] if k < len(sb) else None}")
    ya, yb = symbols(a), symbols(b)
    only_a = ya - yb
    only_b = {y for y in yb - ya if "!turbolib.anchor." not in y}
    anchors = sum(1 for y in yb if "!turbolib.anchor." in y)
    ok &= not only_a and not only_b
    lines.append(f"  symbols: {len(ya)} vs {len(yb)}; only in original {len(only_a)}, only in folded {len(only_b)} "
                 f"(+{anchors} anchors, expected)")
    for y in sorted(only_a)[:5]:
        lines.append(f"    only original: {y}")
    for y in sorted(only_b)[:5]:
        lines.append(f"    only folded: {y}")
    fa, fb = functions(a), functions(b)
    differ = [f for f in fa if f in fb and fa[f] != fb[f]]
    missing = [f for f in fa if f not in fb]
    ok &= not differ and not missing
    lines.append(f"  functions: {len(fa)} vs {len(fb)}; {len(differ)} differ, {len(missing)} missing")
    for f in differ[:5]:
        lines.append(f"    differs: {f}")
    ia, ib = init_order(a), init_order(b)
    ok &= ia == ib
    lines.append(f"  initializers: {len(ia)}, {'the same order' if ia == ib else 'DIFFERENT'}")
    sa, sb = sections(a), sections(b)
    lines.append("  sections: " + ", ".join(f"{s} {sa[s]:#x}->{sb.get(s, 0):#x}" for s in sa if sa[s] != sb.get(s, 0)))
    return ok, "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--folded", required=True)
    ap.add_argument("--kind", default="llvm-ar")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    tmp = tempfile.mkdtemp(prefix="relink-macho-")
    configs = [(t, arch) for t in build.TOOLS for arch in build.ARCHS]
    with ThreadPoolExecutor(len(configs)) as ex:
        results = list(ex.map(lambda c: compare(c[0], c[1], a.kind, a.folded, tmp), configs))
    for _, text in results:
        print(text)
    good = sum(1 for ok, _ in results if ok)
    print(f"{good} of {len(results)} links equivalent")
    if a.keep:
        print(f"images in {tmp}")
    sys.exit(0 if good == len(results) else 1)


if __name__ == "__main__":
    main()
