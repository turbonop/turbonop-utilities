#!/usr/bin/env python3
"""Links programs of a corpus build against its own libraries and against
folded ones, with each linker, and compares what the programs do and which
archive members each link loads.

    relink.py --build ~/tlcorpus/build-gcc --folded DIR [--out DIR]
              [--linkers bfd,gold,lld,mold] [--programs llvm-nm,...]

DIR holds the folded libraries under the build's names (lib/libX.a ->
DIR/libX.a); a library not there is linked as the build made it. Each
program's link command comes from `ninja -t commands`; it is rerun with
-fuse-ld=<linker> (options a linker doesn't take dropped), once with the
build's libraries and once with the folded ones, with -Wl,--trace, which
lists the archive members each link loads. Then each pair of programs is
run on the same inputs (the program's own test commands, below), and
their standard output and exit status compared.

Members loaded: under --pull=preserve, a folded link may load fewer of
the original members (a member that gave a name up is no longer loaded
for it alone: see docs/turbolib.md), plus synthesized members, and never
others. Both sets are printed when they differ.
"""

import argparse
import os
import re
import shlex
import subprocess
import sys

# What each program is run on (from the build directory); {lib} is a big
# library of the build.
RUNS = {
    "llvm-nm": [["{bin}", "-C", "lib/libLLVMDemangle.a"], ["{bin}", "--size-sort", "-S", "lib/libLLVMBinaryFormat.a"]],
    "llvm-readobj": [["{bin}", "--sections", "--symbols", "lib/libLLVMDemangle.a"]],
    "llvm-objdump": [["{bin}", "-d", "-r", "lib/libLLVMDemangle.a"]],
    "llvm-dwarfdump": [["{bin}", "--debug-info", "lib/libLLVMDemangle.a"]],
    "llvm-ar": [["{bin}", "t", "lib/libLLVMSupport.a"]],
    "llvm-tblgen": [["{bin}", "--version"]],
}


def link_command(build, program):
    text = subprocess.run(
        ["ninja", "-C", build, "-t", "commands", "bin/" + program],
        capture_output=True, text=True, check=True,
    ).stdout.strip().splitlines()[-1]
    parts = [p.strip() for p in text.split("&&")]
    cmd = [p for p in parts if p and p != ":"][-1]
    return shlex.split(cmd)


def for_linker(argv, linker, out, folded, trace):
    res = []
    skip_next = False
    for a in argv:
        if a.startswith("-fuse-ld="):
            a = "-fuse-ld=" + linker
        if a.startswith("-Wl,--dependency-file=") or a == "-Wl,--color-diagnostics":
            continue
        if linker == "gold" and a == "-Wl,-z,pack-relative-relocs":
            continue
        if a.startswith("lib/lib") and a.endswith(".a") and folded:
            f = os.path.join(folded, os.path.basename(a))
            if os.path.exists(f):
                a = f
        res.append(a)
    i = res.index("-o")
    res[i + 1] = out
    if trace:
        res.append("-Wl,--trace")
        if linker == "bfd":
            # GNU ld lists archive members with the option twice.
            res.append("-Wl,--trace")
    return res


def loaded(trace_text):
    """The archive members a link loaded, as --trace lists them."""
    members = set()
    for line in trace_text.splitlines():
        # lld, gold, mold: lib.a(member.o); GNU ld: (lib.a)member.o.
        m = re.search(r"([^\s()]+\.a)\(([^)]+)\)", line)
        if m:
            members.add((os.path.basename(m.group(1)), m.group(2)))
            continue
        m = re.search(r"\(([^\s()]+\.a)\)(\S+)", line)
        if m:
            members.add((os.path.basename(m.group(1)), m.group(2)))
    return members


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", required=True)
    ap.add_argument("--folded", required=True)
    ap.add_argument("--out")
    ap.add_argument("--linkers", default="bfd,gold,lld,mold")
    ap.add_argument("--programs", default="llvm-nm,llvm-readobj")
    args = ap.parse_args()
    build = os.path.abspath(args.build)
    out = os.path.abspath(args.out or os.path.join(build, "relink"))
    os.makedirs(out, exist_ok=True)
    folded = os.path.abspath(args.folded)
    failures = 0
    for program in args.programs.split(","):
        base = link_command(build, program)
        for linker in args.linkers.split(","):
            results = {}
            for kind, libs in (("orig", None), ("folded", folded)):
                exe = os.path.join(out, f"{program}-{linker}-{kind}")
                argv = for_linker(base, linker, exe, libs, True)
                p = subprocess.run(argv, cwd=build, capture_output=True, text=True)
                if p.returncode != 0:
                    print(f"FAIL {program} {linker} {kind}: link exited {p.returncode}: {p.stderr[-2000:]}")
                    failures += 1
                    results[kind] = None
                    continue
                results[kind] = (exe, loaded(p.stdout + p.stderr), os.path.getsize(exe))
            if not results.get("orig") or not results.get("folded"):
                continue
            (oexe, omem, osize), (fexe, fmem, fsize) = results["orig"], results["folded"]
            synthesized = {m for m in fmem if m[1].startswith("__turbolib.")}
            extra = fmem - omem - synthesized
            missing = omem - fmem
            line = f"{program} {linker}: {len(omem)} members loaded, folded {len(fmem - synthesized)} (+{len(synthesized)} synthesized), size {osize} -> {fsize}"
            same = True
            if extra:
                same = False
                line += f"; {len(extra)} OTHER members loaded: {sorted(extra)[:5]}"
            if missing:
                line += f"; {len(missing)} not loaded: {sorted(missing)[:5]}"
            # The images' symbols: every function and object, by name, type,
            # and size (addresses move), but the synthesized members' own.
            def symbols(exe):
                p = subprocess.run(["nm", "-S", "--defined-only", exe], capture_output=True, text=True)
                out = set()
                for l in p.stdout.splitlines():
                    f = l.split()
                    if len(f) == 4 and not f[3].startswith("__turbolib."):
                        out.add((f[3], f[2], f[1]))
                return out
            osyms, fsyms = symbols(oexe), symbols(fexe)
            if osyms != fsyms:
                same = False
                only_o = sorted(osyms - fsyms)
                only_f = sorted(fsyms - osyms)
                line += f"; symbols differ: {len(only_o)} only in the original ({only_o[:3]}), {len(only_f)} only in the folded ({only_f[:3]})"
            else:
                line += f"; {len(osyms)} symbols the same"
            for run in RUNS.get(program, []):
                outs = []
                for exe in (oexe, fexe):
                    argv = [exe if a == "{bin}" else a for a in run]
                    p = subprocess.run(argv, cwd=build, capture_output=True)
                    outs.append((p.returncode, p.stdout))
                if outs[0] != outs[1]:
                    same = False
                    line += f"; output of {' '.join(run[1:])} differs ({len(outs[0][1])} vs {len(outs[1][1])} bytes, exit {outs[0][0]} vs {outs[1][0]})"
                else:
                    line += f"; {' '.join(run[1:])}: same ({len(outs[0][1])} bytes)"
            print(("ok   " if same else "FAIL ") + line)
            if not same:
                failures += 1
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
