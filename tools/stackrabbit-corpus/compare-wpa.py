"""Compares Stackrabbit's numbers with WPA's on the trace corpus.

    python tools/stackrabbit-corpus/compare-wpa.py [trace.etl ...]

For each trace (by default, every .etl in the corpus), WPA's exporter
(wpaexporter, from the Windows Performance Toolkit) writes WPA's tables as
CSV, and `stackrabbit query` answers the same questions; the script prints
where they differ. The profile it exports with is cut, at run time, from
one installed with WPA (AppLaunch.wpaProfile), so nothing of WPA's is kept
in this repository.

Compared, per process (by name and PID):

- CPU Usage (Sampled): the samples (WPA's Count), and their weight.
- CPU Usage (Precise): the time running (WPA's CPU Usage (ms), summed over
  the process's switches in).

Everything it writes goes to c:\\jm\\dev-private\\stackrabbit\\wpa, beside
the corpus: what's exported from a trace stays on this machine.
"""

import csv
import io
import json
import os
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

PRIVATE = Path(r"c:\jm\dev-private\stackrabbit")
CORPUS = PRIVATE / "corpus"
OUT = PRIVATE / "wpa"
WPT = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / (
    "Windows Kits/10/Windows Performance Toolkit"
)
REPO = Path(__file__).resolve().parents[2]
STACKRABBIT = REPO / "target" / "release" / "stackrabbit.exe"

# The graphs kept from AppLaunch.wpaProfile, by graph GUID and preset name.
GRAPHS = {
    ("b855361e-7be0-4bc8-a754-3e8507715ca5", "Breakdown by Process, Thread, Activity, Stack"),
    ("c58f5fea-0319-4046-932d-e695ebe20b47", "By Process"),
}
NS = "http://tempuri.org/SerializableElement.xsd"


def profile() -> Path:
    """A profile with only the graphs compared (written once)."""
    path = OUT / "compare.wpaProfile"
    if path.exists():
        return path
    ET.register_namespace("", NS)
    ET.register_namespace("xsi", "http://www.w3.org/2001/XMLSchema-instance")
    ET.register_namespace("xsd", "http://www.w3.org/2001/XMLSchema")
    tree = ET.parse(WPT / "Catalog" / "AppLaunch.wpaProfile")
    root = tree.getroot()
    q = lambda tag: f"{{{NS}}}{tag}"
    # No regions files (those it names aren't beside the copy).
    for parent in root.iter():
        for child in list(parent):
            if child.tag == q("FileReferences"):
                parent.remove(child)
    kept = set()
    for view in root.iter(q("View")):
        for graph in list(view.iter(q("Graph"))):
            preset = graph.find(q("Preset"))
            key = (graph.get("Guid"), preset.get("Name") if preset is not None else None)
            parent = next(p for p in view.iter() if graph in list(p))
            if key in GRAPHS and key not in kept:
                kept.add(key)
                # Every row, not just those the preset's filter keeps; and
                # the precise table's CPU time, which it hides.
                preset.attrib.pop("InitialFilterQuery", None)
                for col in preset.iter(q("Column")):
                    if col.get("Name") == "CPU Usage (in view)":
                        col.set("IsVisible", "true")
            else:
                parent.remove(graph)
    # Views left with no graphs go.
    for parent in root.iter():
        for child in list(parent):
            if child.tag == q("View") and child.find(f".//{q('Graph')}") is None:
                parent.remove(child)
    if kept != GRAPHS:
        sys.exit(f"compare-wpa: AppLaunch.wpaProfile lacks {GRAPHS - kept}")
    OUT.mkdir(parents=True, exist_ok=True)
    tree.write(path, encoding="utf-8", xml_declaration=True)
    return path


def export(trace: Path) -> Path:
    """WPA's tables for `trace` (exported once, again if the trace changes)."""
    out = OUT / trace.stem
    stamp = out / ".exported"
    if stamp.exists() and stamp.stat().st_mtime >= trace.stat().st_mtime:
        return out
    out.mkdir(parents=True, exist_ok=True)
    for f in out.glob("*.csv"):
        f.unlink()
    r = subprocess.run(
        [str(WPT / "wpaexporter.exe"), "-i", str(trace), "-profile", str(profile()), "-outputfolder", str(out)],
        capture_output=True,
        text=True,
    )
    # (A trace without the tables, as one with no CPU samples, gets none
    # written: not an error. The exporter's log is kept beside them.)
    (out / "wpaexporter.log").write_text(r.stdout + r.stderr, encoding="utf-8")
    stamp.touch()
    return out


def number(s: str) -> float:
    return float(s.replace(",", "")) if s.strip() else 0.0


def wpa_csv(folder: Path, pattern: str) -> list[dict]:
    files = sorted(folder.glob(pattern))
    if not files:
        return []
    with open(files[0], encoding="utf-8-sig", newline="") as f:
        return list(csv.DictReader(f))


def process_key(name_and_pid: str) -> tuple[str, int] | None:
    """(name, PID) from WPA's "name <package> (pid)", without the package
    or service WPA adds to some names."""
    m = re.fullmatch(r"(.*?)(?: <[^>]*>)? \((-?\d+)\)", name_and_pid.strip())
    return (m.group(1), int(m.group(2))) if m else None


def by_process(rows: list[dict], process: str, thread: str, values) -> dict:
    """`values(row)` summed by process. WPA's export mixes levels: a process
    the preset leaves collapsed has a row of its own (its thread column
    empty); one it expands, only rows below it. Each process's own row is
    taken if it has one, else the sum of the rows below it."""
    own, below = {}, {}
    for row in rows:
        k = process_key(row.get(process, ""))
        if not k:
            continue
        into = own if not row.get(thread) else below
        into.setdefault(k, []).append(values(row))
    out = {}
    for k in set(own) | set(below):
        parts = own.get(k) or below[k]
        out[k] = [sum(v[i] for v in parts) for i in range(len(parts[0]))]
    return out


def wpa_numbers(folder: Path) -> dict:
    sampled = by_process(
        wpa_csv(folder, "CPU_Usage_(Sampled)_*.csv"),
        "Process",
        "Thread ID",
        lambda r: (int(number(r["Count"])), number(r["Weight (ms)"])),
    )
    rows = wpa_csv(folder, "CPU_Usage_(Precise)_*.csv")
    column = next((c for c in (rows[0] if rows else {}) if c and c.startswith("CPU Usage (in view)")), None)
    precise = {}
    if column:
        precise = {
            k: v[0]
            for k, v in by_process(rows, "New Process", "New Thread Id", lambda r: (number(r[column]),)).items()
        }
    return {"sampled": sampled, "precise": precise}


def query(trace: Path, method: str) -> dict:
    r = subprocess.run([str(STACKRABBIT), "query", str(trace), method], capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"compare-wpa: stackrabbit query {method} failed on {trace.name}: {r.stderr}")
    return json.loads(r.stdout)


def ours(trace: Path) -> dict:
    sampled = {}
    for p in query(trace, "processes")["rows"]:
        k = (p["name"], p["pid"])
        s = sampled.setdefault(k, [0, 0.0])
        s[0] += p["samples"]
        s[1] += p["cpuMs"]
    precise = {}
    for t in query(trace, "threads")["rows"]:
        if t["pid"] is None:
            continue
        k = (t["process"], t["pid"])
        precise[k] = precise.get(k, 0.0) + t["runningMs"]
    return {"sampled": sampled, "precise": precise}


def compare(trace: Path) -> list[str]:
    wpa = wpa_numbers(export(trace))
    if not wpa["sampled"] and not wpa["precise"]:
        return ["  WPA has no CPU tables for it (no samples or context switches)"]
    us = ours(trace)
    notes = []
    w, o = wpa["sampled"], us["sampled"]
    keys = sorted(set(w) | {k for k, v in o.items() if v[0]})
    count_diff = [(k, w.get(k, [0, 0])[0], o.get(k, [0, 0])[0]) for k in keys]
    count_diff = [d for d in count_diff if d[1] != d[2]]
    total_w = sum(v[0] for v in w.values())
    total_o = sum(v[0] for v in o.values())
    notes.append(
        f"  sampled: {len(w)} processes in WPA, {sum(1 for v in o.values() if v[0])} in Stackrabbit; "
        f"samples {total_w} vs {total_o}; {len(count_diff)} processes' counts differ"
    )
    for k, a, b in sorted(count_diff, key=lambda d: -abs(d[1] - d[2]))[:8]:
        notes.append(f"    {k[0]} ({k[1]}): WPA {a}, Stackrabbit {b}")
    # Weights, but the idle process's: WPA counts time no sample covers as
    # idle (see turbonop_trace_analysis::weights).
    idle = lambda k: k[1] == 0
    weight_diff = [
        (k, w.get(k, [0, 0.0])[1], o.get(k, [0, 0.0])[1])
        for k in keys
        if not idle(k)
    ]
    # (WPA prints weights to 0.1 us.)
    weight_diff = [d for d in weight_diff if abs(d[1] - d[2]) > 0.0005]
    ws = sum(v[1] for k, v in w.items() if not idle(k))
    os_ = sum(v[1] for k, v in o.items() if not idle(k))
    notes.append(
        f"  sampled weight, but Idle: WPA {ws:,.4f} ms, Stackrabbit {os_:,.4f} ms; "
        f"{len(weight_diff)} processes differ"
    )
    for k, a, b in sorted(weight_diff, key=lambda d: -abs(d[1] - d[2]))[:8]:
        notes.append(f"    {k[0]} ({k[1]}): WPA {a:,.4f} ms, Stackrabbit {b:,.4f} ms")
    wi = sum(v[1] for k, v in w.items() if idle(k))
    oi = sum(v[1] for k, v in o.items() if idle(k))
    notes.append(f"  Idle's weight: WPA {wi:,.1f} ms, Stackrabbit {oi:,.1f} ms (only what samples cover)")
    w, o = wpa["precise"], us["precise"]
    if w:
        keys = sorted(set(w) | {k for k, v in o.items() if v})
        diffs = [(k, w.get(k, 0.0), o.get(k, 0.0)) for k in keys]
        # Off by more than a microsecond per switch's rounding, or 0.01%.
        big = [d for d in diffs if abs(d[1] - d[2]) > max(0.01, 1e-4 * max(d[1], d[2]))]
        notes.append(
            f"  precise: {len(w)} processes in WPA; running {sum(w.values()):,.1f} vs {sum(o.values()):,.1f} ms; "
            f"{len(big)} processes differ"
        )
        for k, a, b in sorted(big, key=lambda d: -abs(d[1] - d[2]))[:8]:
            notes.append(f"    {k[0]} ({k[1]}): WPA {a:,.3f} ms, Stackrabbit {b:,.3f} ms")
    else:
        notes.append("  precise: WPA's export has no CPU Usage (in view) column")
    return notes


def main() -> None:
    traces = [Path(a) for a in sys.argv[1:]] or sorted(CORPUS.glob("*.etl"))
    if not STACKRABBIT.exists():
        sys.exit(f"compare-wpa: build {STACKRABBIT} first (cargo build --release -p stackrabbit)")
    for trace in traces:
        print(trace.name)
        for line in compare(trace):
            print(line)
        sys.stdout.flush()


if __name__ == "__main__":
    main()
