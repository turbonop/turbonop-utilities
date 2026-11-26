#!/usr/bin/env python3
"""Proves the subprograms `compare-images` reports as described differently
in a folded program (\"subprograms whose DIEs differ\") are another
translation unit's copy: the one turbolib's synthesized member describes
them by (the first member's), where the original link kept another's.

    witness.py --build ~/tlcorpus/build-gcc --relink DIR --turbolib PATH
               [--programs llvm-nm,...] [--linkers bfd,gold,lld,mold]

DIR is relink.py's --out, run with --compare-images: it holds each
program's original and folded links and their comparison
(<program>-<linker>-compare.txt, from `compare-images --examples=0`, so
that every subprogram is listed). For each subprogram listed, the unit
the folded program describes it by is named in the report (\"units A and
B\": B). For each such unit, the original program is linked again with
that unit's object first on the command line, so that every linker keeps
its copy of each COMDAT (and GNU ld and gold describe the others' too);
then the witness is compared with the folded program. A subprogram is
proven when the witness describes it as the folded program does (it is
not listed as differing). What the witness changes otherwise (more
members, other copies kept) is not looked at.
"""

import argparse
import collections
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from relink import for_linker, link_command  # noqa: E402

CATEGORY = "subprograms whose DIEs differ"


def differing(report_text, category=CATEGORY):
    """The subprograms a report lists under a category: {name: the second
    program's unit, if named}."""
    out = {}
    current = None
    for line in report_text.splitlines():
        m = re.match(r"(?:FAILED|expected|not checked): (.*): \d+$", line)
        if m:
            current = m.group(1)
            continue
        if not line.startswith("    "):
            current = None
            continue
        if current != category:
            continue
        m = re.match(r"    (\S+): (?:\(units (.*?) and (.*?)\) )?", line)
        if m:
            out[m.group(1)] = m.group(3)
        else:
            out[line.strip()] = None
    return out


def objects_by_source(build):
    with open(os.path.join(build, "compile_commands.json")) as f:
        db = json.load(f)
    out = {}
    for e in db:
        o = e.get("output")
        if o is None:
            args = e.get("arguments") or e["command"].split()
            o = args[args.index("-o") + 1]
        out[os.path.normpath(os.path.join(e["directory"], e["file"]))] = os.path.join(e["directory"], o)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", required=True)
    ap.add_argument("--relink", required=True)
    ap.add_argument("--turbolib", required=True)
    ap.add_argument("--programs", default="llvm-nm,llvm-readobj")
    ap.add_argument("--linkers", default="bfd,gold,lld,mold")
    args = ap.parse_args()
    build = os.path.abspath(args.build)
    relink = os.path.abspath(args.relink)
    objects = objects_by_source(build)
    unproven_total = 0
    for program in args.programs.split(","):
        base = link_command(build, program)
        for linker in args.linkers.split(","):
            report = os.path.join(relink, f"{program}-{linker}-compare.txt")
            with open(report) as f:
                subs = differing(f.read())
            folded = os.path.join(relink, f"{program}-{linker}-folded")
            by_unit = collections.defaultdict(list)
            for name, unit in subs.items():
                by_unit[unit].append(name)
            proven, unproven = [], []
            for unit, names in sorted(by_unit.items(), key=lambda x: str(x[0])):
                obj = objects.get(os.path.normpath(unit)) if unit else None
                if obj is None:
                    unproven += [(n, "no unit named, or no object for it") for n in names]
                    continue
                exe = os.path.join(relink, f"{program}-{linker}-witness")
                argv = for_linker(base, linker, exe, None, False)
                # (One of the program's own objects moves to the front.)
                rel = os.path.relpath(obj, build)
                argv = [a for a in argv if os.path.normpath(os.path.join(build, a)) != os.path.normpath(obj)
                        and a != rel]
                first = next(i for i, a in enumerate(argv) if a.endswith(".o"))
                argv.insert(first, obj)
                p = subprocess.run(argv, cwd=build, capture_output=True, text=True)
                if p.returncode != 0:
                    unproven += [(n, f"witness link failed: {p.stderr[-300:]}") for n in names]
                    continue
                r = subprocess.run([args.turbolib, "compare-images", "--examples=0", exe, folded],
                                   capture_output=True, text=True)
                still = differing(r.stdout)
                rows = differing(r.stdout, "subprograms whose line rows differ")
                only = set(differing(r.stdout, "subprograms only in the first program")) | set(
                    differing(r.stdout, "subprograms only in the second program"))
                for n in names:
                    if n in still or n in rows or n in only:
                        unproven.append((n, f"the witness with {os.path.basename(obj)} differs too"))
                    else:
                        proven.append((n, os.path.basename(obj)))
                os.remove(exe)
            print(f"{program} {linker}: {len(subs)} differing; {len(proven)} proven another unit's copy"
                  + (f"; {len(unproven)} NOT" if unproven else ""))
            for n, why in unproven:
                print(f"    {n}: {why}")
            unproven_total += len(unproven)
    sys.exit(1 if unproven_total else 0)


if __name__ == "__main__":
    main()
