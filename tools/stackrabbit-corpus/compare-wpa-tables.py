"""Compares Stackrabbit's tables with WPA's, row by row, on the corpus.

    python tools/stackrabbit-corpus/compare-wpa-tables.py [trace.etl ...]

For each trace (by default, every .etl in the corpus), WPA's exporter
writes the tables WPA's own WpaRuleMatchExporter.wpaProfile exports (its
RuleEngine presets list a table's rows one by one), and `stackrabbit
query` answers for the same; the script pairs the rows and prints what
differs. compare-wpa.py does the same for CPU usage.

Compared, where the trace has them:

- Disk Usage: each I/O (by completion time, size and type): its thread,
  I/O time, disk service time and path.
- DPC/ISR: each run (by processor and start): its duration and module.
- VirtualAlloc Commit Lifetimes: each commit (by address and time): its
  size and decommit time.
- Pool: each tag's allocations made in the trace.
- Heap Allocations: each allocation (by address and time) WPA lists.
- Virtual Memory Snapshots: each snapshot's totals.
- Marks: each mark's time and text.

What it exports stays in c:\\jm\\dev-private\\stackrabbit\\wpa, beside the
corpus.
"""

import csv
import json
import os
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

PRIVATE = Path(r"c:\jm\dev-private\stackrabbit")
CORPUS = PRIVATE / "corpus"
OUT = PRIVATE / "wpa"
WPT = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / (
    "Windows Kits/10/Windows Performance Toolkit"
)
REPO = Path(__file__).resolve().parents[2]
STACKRABBIT = REPO / "target" / "release" / "stackrabbit.exe"


def export(trace: Path) -> Path:
    """WPA's RuleEngine tables for `trace` (exported once)."""
    out = OUT / f"rules-{trace.stem}"
    stamp = out / ".exported"
    if stamp.exists() and stamp.stat().st_mtime >= trace.stat().st_mtime:
        return out
    out.mkdir(parents=True, exist_ok=True)
    for f in out.glob("*.csv"):
        f.unlink()
    profile = WPT / "Catalog" / "WpaRuleMatchExporter.wpaProfile"
    r = subprocess.run(
        [str(WPT / "wpaexporter.exe"), "-i", str(trace), "-profile", str(profile), "-outputfolder", str(out)],
        capture_output=True,
        text=True,
    )
    # (It stops at a table the trace has no data for, and says so; what it
    # wrote before is kept, with its log.)
    (out / "log.txt").write_text(r.stdout + r.stderr, encoding="utf-8")
    stamp.touch()
    return out


def rows(folder: Path, name: str) -> list[dict] | None:
    path = folder / name
    if not path.exists():
        return None
    with open(path, encoding="utf-8-sig", newline="") as f:
        return list(csv.DictReader(f))


def number(s: str | None) -> float:
    return float(s.replace(",", "")) if s and s.strip() else 0.0


def query(trace: Path, method: str, **params) -> dict:
    args = [str(STACKRABBIT), "query", str(trace), method] + [f"{k}={v}" for k, v in params.items()]
    r = subprocess.run(args, capture_output=True, text=True, encoding="utf-8")
    if r.returncode != 0:
        sys.exit(f"compare-wpa-tables: {' '.join(args[2:])}: {r.stderr}")
    return json.loads(r.stdout)


def tick(seconds: float) -> int:
    """A time in WPA's 0.1 us, as a key."""
    return round(seconds * 1e7)


def disk(trace: Path, wpa: list[dict]) -> list[str]:
    us = query(trace, "disk", limit=10**7, groupLimit=10**6, by="process")
    ours = defaultdict(list)
    for x in us["ios"]:
        ours[(round(x["completeNs"] / 100), x["size"], x["kind"])].append(x)
    missing, differ, paths = 0, [], 0
    for r in wpa:
        o = ours.get((tick(number(r["Complete Time (s)"])), int(number(r["Size (B)"])), r["IO Type"]))
        if not o:
            missing += 1
            continue
        x = o.pop(0)
        tid = "-1" if x["tid"] is None else str(x["tid"])
        io, service = number(r["IO Time (µs)"]), number(r["Disk Service Time (µs)"])
        if abs(x["ioNs"] / 1e3 - io) > 0.05 or abs(x["serviceNs"] / 1e3 - service) > 0.05 or tid != r["Thread ID"]:
            differ.append((r["IO Type"], r["Complete Time (s)"], io, x["ioNs"] / 1e3, service, x["serviceNs"] / 1e3))
        # (Paths that differ only in case: WPA names a file by another of
        # its names. A file WPA doesn't know it calls Unknown.)
        name = r["Path Name"] or ""
        if not name.startswith("Unknown") and name.lower() != (x["file"] or "").lower():
            paths += 1
    extra = sum(len(v) for v in ours.values())
    notes = [f"  disk: {len(wpa)} I/Os in WPA; {missing} not in Stackrabbit, {extra} only in it; "
             f"{len(differ)} differ; {paths} paths differ"]
    notes += [f"    {d}" for d in differ[:5]]
    return notes


def dpc(trace: Path, wpa: list[dict]) -> list[str]:
    us = query(trace, "dpc", limit=10**7, groupLimit=10**6, by="module")
    ours = {(x["processor"], round(x["startNs"] / 100)): x for x in us["runs"]}
    runs = {}
    for r in wpa:
        runs.setdefault((int(r["CPU"]), tick(number(r["DPC/ISR Enter Time (s)"]))), r)
    missing, differ, modules = 0, 0, Counter()
    for k, r in runs.items():
        x = ours.pop(k, None)
        if x is None:
            missing += 1
            continue
        if abs(x["durationNs"] - number(r["Inclusive Duration (ms)"]) * 1e6) > 100:
            differ += 1
        if r["Module"] != x["module"]:
            modules[(r["Module"], x["module"])] += 1
    # (WPA leaves out each processor's first run; and gives KMDF drivers'
    # runs, whose routine is Wdf01000.sys's dispatcher, to the driver.)
    firsts = {}
    for x in us["runs"]:
        c = x["processor"]
        firsts[c] = min(firsts.get(c, x["startNs"]), x["startNs"])
    first = sum(1 for (c, t), x in ours.items() if x["startNs"] == firsts.get(c))
    kmdf = sum(n for (a, b), n in modules.items() if b == "Wdf01000.sys")
    notes = [f"  DPC/ISR: {len(runs)} runs in WPA; {missing} not in Stackrabbit, {len(ours)} only in it "
             f"({first} a processor's first, as WPA leaves out); "
             f"{differ} durations differ; {sum(modules.values())} modules differ ({kmdf} KMDF dispatcher's)"]
    notes += [f"    WPA {a}, Stackrabbit {b}: {n}" for (a, b), n in modules.most_common(4)]
    return notes


def vmem(trace: Path, wpa: list[dict]) -> list[str]:
    us = query(trace, "vmem", limit=10**7, groupLimit=10**6, by="process")
    ours = defaultdict(list)
    for x in us["lifetimes"]:
        ours[(int(x["base"], 16), round(x["commitNs"] / 100))].append(x)
    missing, sizes, decommits = 0, 0, 0
    for r in wpa:
        o = ours.get((int(r["Address"], 16), tick(number(r["Commit Time (s)"]))))
        if not o:
            missing += 1
            continue
        x = o.pop(0)
        if abs(x["size"] / 2**20 - number(r["Size (MB)"])) > 0.0006:
            sizes += 1
        # (WPA's never is the largest time it can show.)
        decommit = number(r["Decommit Time (s)"])
        never = decommit > 1e9
        if never != (x["decommitNs"] is None) or (not never and abs(x["decommitNs"] / 1e9 - decommit) > 2e-7):
            decommits += 1
    extra = sum(len(v) for v in ours.values())
    return [f"  VirtualAlloc: {len(wpa)} commits in WPA; {missing} not in Stackrabbit, {extra} only in it; "
            f"{sizes} sizes and {decommits} decommits differ"]


def pool(trace: Path, wpa: list[dict]) -> list[str]:
    us = query(trace, "pool", limit=0, groupLimit=10**6, by="tag")
    # (Allocations in the trace: WPA's AIFI and AIFO. It also lists frees
    # of allocations from before, which Stackrabbit leaves out.)
    tags = Counter(r["Pool Tag"].strip() for r in wpa if r["Type"] in ("AIFI", "AIFO"))
    ours = {g["label"].strip(): g["allocs"] for g in us["groups"]}
    differ = [(t, n, ours.get(t)) for t, n in tags.items() if ours.get(t) != n]
    notes = [f"  pool: {len(tags)} tags in WPA, {len(ours)} in Stackrabbit; {len(differ)} tags' allocations differ"]
    notes += [f"    {d}" for d in differ[:5]]
    return notes


def heap(trace: Path, wpa: list[dict]) -> list[str]:
    us = query(trace, "heap", limit=10**7, groupLimit=10**6, by="process")
    ours = defaultdict(list)
    for x in us["blocks"]:
        ours[(int(x["address"], 16), round(x["allocNs"] / 100))].append(x)
    listed = [r for r in wpa if r["Type"]]
    missing = 0
    for r in listed:
        o = ours.get((int(r["Address"], 16), tick(number(r["AllocTime (s)"]))))
        if o:
            o.pop(0)
        else:
            missing += 1
    return [f"  heap: {len(listed)} allocations in WPA; {missing} not in Stackrabbit"]


def memory(trace: Path, wpa: list[dict]) -> list[str]:
    us = query(trace, "memory", limit=10**7, groupLimit=10**6)
    totals = defaultdict(lambda: [0, 0.0, 0.0, 0.0])
    for r in wpa:
        t = totals[tick(number(r["Time (s)"]))]
        t[0] += 1
        t[1] += number(r["Working Set (B)"])
        t[2] += number(r["Private Working Set (B)"])
        t[3] += number(r["Commit (B)"])
    differ = 0
    for s in us["series"]:
        t = totals.get(round(s["timeNs"] / 100))
        if not t or tuple(t) != (s["processes"], s["workingSet"], s["privateWorkingSet"], s["commit"]):
            differ += 1
    return [f"  memory: {len(totals)} snapshots in WPA, {len(us['series'])} in Stackrabbit; {differ} differ"]


def marks(trace: Path, wpa: list[dict]) -> list[str]:
    us = query(trace, "marks")
    theirs = [(tick(number(r["Time (s)"])), r["Mark"]) for r in wpa]
    ours = [(round(m["timeNs"] / 100), m["mark"]) for m in us["rows"]]
    return [f"  marks: {len(theirs)} in WPA, {len(ours)} in Stackrabbit; "
            f"{'the same' if sorted(theirs) == sorted(ours) else 'they differ'}"]


TABLES = [
    ("Disk_Usage_RuleEngine_(2).csv", disk),
    ("DPC_ISR_RuleEngine.csv", dpc),
    ("VirtualAlloc_Commit_LifeTimes_RuleEngine.csv", vmem),
    ("Pool_Graphs_RuleEngine.csv", pool),
    ("Heap_Allocations_RuleEngine.csv", heap),
    ("Virtual_Memory_Snapshots_RuleEngine.csv", memory),
    ("Marks_Summary_Table_RuleEngine.csv", marks),
]


def main() -> None:
    traces = [Path(a) for a in sys.argv[1:]] or sorted(CORPUS.glob("*.etl"))
    if not STACKRABBIT.exists():
        sys.exit(f"compare-wpa-tables: build {STACKRABBIT} first (cargo build --release -p stackrabbit)")
    for trace in traces:
        print(trace.name)
        folder = export(trace)
        for name, compare in TABLES:
            wpa = rows(folder, name)
            if wpa:
                for line in compare(trace, wpa):
                    print(line)
        sys.stdout.flush()


if __name__ == "__main__":
    main()
