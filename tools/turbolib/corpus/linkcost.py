"""Measures what a program's link costs against several sets of a Ninja
build's libraries: the link's time and peak memory, the PDB's size and
records, and which members it loads from each library.

    python linkcost.py <build dir> <program>... --libs NAME=DIR...
                       [--linker link|lld]... [--opt REF|NOREF]
                       [--rounds N] [--keep DIR] [--json FILE]
                       [--link LINK] [--lld LLD]

Each program is linked as relink.py links it (its build statement, with
/Brepro, /INCREMENTAL:NO, /DEBUG:FULL, and the /OPT given; REF by
default), once for each --libs set: NAME=build (or a DIR of `-`) is the
build's own libraries; otherwise each library lib\\X.lib is DIR\\X.lib
where that file exists. Each link is run --rounds times (default 3), and
the fastest round's time and the largest peak memory are reported.

From each PDB: its size, the TPI and IPI streams' record counts and
bytes, and its modules (DBI), by library: how many members the link
loaded from each, and which of them are turbolib's precompiled-types
members (turbolib-types-*.obj), whose records the link merged whether it
used them or not.
"""
import argparse
import ctypes
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import time
from ctypes import wintypes

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import relink  # noqa: E402


class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD),
                ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t)]


def run_measured(args, cwd):
    """Runs a linker: (seconds, peak working set in bytes)."""
    start = time.perf_counter()
    p = subprocess.Popen(args, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    out, _ = p.communicate()
    seconds = time.perf_counter() - start
    counters = PROCESS_MEMORY_COUNTERS()
    counters.cb = ctypes.sizeof(counters)
    ok = ctypes.windll.psapi.GetProcessMemoryInfo(int(p._handle), ctypes.byref(counters),
                                                  counters.cb)
    if p.returncode != 0:
        raise SystemExit(f"link failed ({p.returncode}): {out.decode(errors='replace')}")
    return seconds, counters.PeakWorkingSetSize if ok else 0


class Msf:
    """A PDB's streams (MSF 7.0)."""

    def __init__(self, path):
        with open(path, "rb") as f:
            self.data = f.read()
        d = self.data
        if not d.startswith(b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0"):
            raise SystemExit(f"{path}: not an MSF 7.0 PDB")
        self.block, _, _, dir_bytes, _, map_addr = struct.unpack_from("<6I", d, 32)
        n = (dir_bytes + self.block - 1) // self.block
        dir_blocks = struct.unpack_from(f"<{n}I", d, map_addr * self.block)
        directory = b"".join(d[b * self.block:(b + 1) * self.block] for b in dir_blocks)
        count = struct.unpack_from("<I", directory, 0)[0]
        sizes = struct.unpack_from(f"<{count}I", directory, 4)
        at = 4 + 4 * count
        self.streams = []
        for size in sizes:
            if size == 0xFFFFFFFF:
                size = 0
            k = (size + self.block - 1) // self.block
            blocks = struct.unpack_from(f"<{k}I", directory, at)
            at += 4 * k
            self.streams.append((size, blocks))

    def stream(self, i):
        size, blocks = self.streams[i]
        b = self.block
        return b"".join(self.data[x * b:(x + 1) * b] for x in blocks)[:size]


def pdb_facts(path):
    msf = Msf(path)
    facts = {"bytes": os.path.getsize(path)}
    for name, i in (("tpi", 2), ("ipi", 4)):
        s = msf.stream(i)
        _, _, begin, end, record_bytes = struct.unpack_from("<5I", s, 0)
        facts[name] = end - begin
        facts[name + "_bytes"] = record_bytes
    dbi = msf.stream(3)
    mod_size = struct.unpack_from("<i", dbi, 24)[0]
    at, end = 64, 64 + mod_size
    modules = []
    while at < end:
        at += 64
        z = dbi.index(b"\0", at)
        module = dbi[at:z].decode(errors="replace")
        at = z + 1
        z = dbi.index(b"\0", at)
        obj = dbi[at:z].decode(errors="replace")
        at = (z + 1 + 3) & ~3
        modules.append((module, obj))
    by_lib = {}
    pchs = {}
    for module, obj in modules:
        if obj.lower().endswith(".lib"):
            lib = os.path.basename(obj)
            by_lib[lib] = by_lib.get(lib, 0) + 1
            if os.path.basename(module).startswith("turbolib-types-"):
                pchs.setdefault(lib, []).append(os.path.basename(module))
    facts["modules"] = len(modules)
    facts["members_by_library"] = by_lib
    facts["pch_modules"] = pchs
    return facts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("build_dir")
    ap.add_argument("programs", nargs="+")
    ap.add_argument("--libs", action="append", required=True, help="NAME=DIR")
    ap.add_argument("--linker", action="append", choices=["link", "lld"])
    ap.add_argument("--opt", default="REF", choices=["REF", "NOREF"], type=str.upper)
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--keep")
    ap.add_argument("--json")
    ap.add_argument("--link", default=relink.LINK)
    ap.add_argument("--lld", default=relink.LLD)
    a = ap.parse_args()
    sets = []
    for spec in a.libs:
        name, _, d = spec.partition("=")
        sets.append((name, None if d in ("", "-", "build") else d))
    scratch = a.keep or tempfile.mkdtemp(prefix="linkcost-")
    mirror = os.path.join(scratch, "mirror")
    results = []
    for target in a.programs:
        program = relink.read_program(a.build_dir, target)
        inputs = relink.relative_inputs(program)
        os.makedirs(os.path.join(mirror, os.path.dirname(target)), exist_ok=True)
        for linker in a.linker or ["link", "lld"]:
            exe = a.link if linker == "link" else a.lld
            for name, libs in sets:
                swapped = 0
                for rel in inputs:
                    src = os.path.join(a.build_dir, rel)
                    if libs and rel.lower().endswith(".lib"):
                        alt = os.path.join(libs, os.path.basename(rel))
                        if os.path.exists(alt):
                            src = alt
                            swapped += 1
                    relink.place(src, os.path.join(mirror, rel))
                best, peak = None, 0
                for _ in range(a.rounds):
                    for ext in (".exe", ".pdb"):
                        stale = os.path.join(mirror, target[:-4] + ext)
                        if os.path.exists(stale):
                            os.remove(stale)
                    s, m = run_measured(
                        [exe] + relink.link_args(program, target, a.opt, "FULL"), mirror)
                    best = s if best is None else min(best, s)
                    peak = max(peak, m)
                facts = pdb_facts(os.path.join(mirror, target[:-4] + ".pdb"))
                facts["exe_bytes"] = os.path.getsize(os.path.join(mirror, target))
                r = {"program": target, "linker": linker, "libs": name, "swapped": swapped,
                     "seconds": best,
                     "peak": peak, **facts}
                results.append(r)
                loaded = sum(facts["members_by_library"].values())
                npch = sum(len(v) for v in facts["pch_modules"].values())
                print(f"{target} {linker} {name} ({swapped} swapped): {best:.2f} s, peak {peak / 2**20:.0f} MiB, "
                      f"PDB {facts['bytes'] / 2**20:.1f} MiB, TPI {facts['tpi']} "
                      f"({facts['tpi_bytes'] / 2**20:.1f} MiB), IPI {facts['ipi']} "
                      f"({facts['ipi_bytes'] / 2**20:.1f} MiB), {loaded} members of "
                      f"{len(facts['members_by_library'])} libraries, {npch} PCH members",
                      flush=True)
                if a.keep:
                    for ext in (".exe", ".pdb"):
                        shutil.copyfile(os.path.join(mirror, target[:-4] + ext), os.path.join(
                            scratch, f"{os.path.basename(target)[:-4]}.{linker}.{name}{ext}"))
    if a.json:
        with open(a.json, "w") as f:
            json.dump(results, f, indent=1)
    if not a.keep:
        shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    main()
