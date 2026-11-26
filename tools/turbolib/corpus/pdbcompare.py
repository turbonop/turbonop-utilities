"""Compares two PDBs' procedures and module symbols structurally, from
llvm-pdbutil's dumps, for programs linked against a library and against its
folded copy.

    python pdbcompare.py <original.pdb> <candidate.pdb> [--pdbutil PATH]
                         [--show N] [--modules REGEX]

Each procedure (S_GPROC32, S_LPROC32, and their _ID forms) becomes one
normalized text: its name, signature, and code size; every record inside
it (locals, frames, blocks, inline sites, def ranges, callees), with type
and id indices replaced by the names llvm-pdbutil prints beside them,
stream offsets (parent, end, next) dropped, code addresses made relative
to the procedure's start, and data addresses dropped; its line tables
(file, line, and offset from the procedure's start); and, for each inline
site, its inlinee's source line and file from the module's inlinee lines
(but not the files its annotations change to, which llvm-pdbutil doesn't
name).
Records outside procedures (S_GDATA32, S_UDT, S_CONSTANT, ...) are compared
the same way, apart from S_OBJNAME, S_COMPILE3, and S_BUILDINFO, which name
the module.

The two PDBs must hold the same multisets of procedures and of other
records. Which module holds each is not compared: a folded function's
module is the synthesized member. The procedures found in modules whose
name matches --modules (default: turbolib's synthesized members) are
counted.
"""
import argparse
import collections
import os
import re
import subprocess
import sys
import tempfile

PDBUTIL = r"C:\jm\dev\llvm-project\build-release\bin\llvm-pdbutil.exe"

PROC_KINDS = {"S_GPROC32", "S_LPROC32", "S_GPROC32_ID", "S_LPROC32_ID",
              "S_GPROC32_DPC", "S_LPROC32_DPC", "S_GPROC32_DPC_ID", "S_LPROC32_DPC_ID"}
OPEN = PROC_KINDS | {"S_BLOCK32", "S_INLINESITE", "S_INLINESITE2", "S_THUNK32",
                     "S_SEPCODE", "S_WITH32"}
CLOSE = {"S_END", "S_PROC_ID_END", "S_INLINESITE_END"}
MODULE_KINDS = {"S_OBJNAME", "S_COMPILE3", "S_COMPILE2", "S_BUILDINFO", "S_ENVBLOCK"}
# The linker's module describes the image's layout, which folding changes
# (COMDATs move).
LAYOUT_KINDS = {"S_SECTION", "S_COFFGROUP"}

MOD = re.compile(r"^\s*Mod (\d+) \| `(.*)`:\s*$")
RECORD = re.compile(r"^\s*(\d+) \| (S_\w+) \[size = \d+\](.*)$")
BLOCK = re.compile(r"^\s*([0-9A-F]{4}):([0-9A-F]{8})-([0-9A-F]{8}), line/addr entries = (\d+)")
ENTRY = re.compile(r"(\d+)\s+([0-9A-F]{8})(!?)")
INLINEE = re.compile(r"^\s*0x([0-9A-F]+) \|\s*(\d+) \| (.*)$")
ADDR = re.compile(r"\b(\d{4}):(\d+)\b")
INDEX = re.compile(r"0x[0-9A-F]{4,} \(")
LINKS = re.compile(r"\b(parent|end|next) = \d+(, )?")
ANNOTATION = re.compile(r"^[0-9A-F]{2,}\s{2,}(?=\S)")
# S_UDT's type is printed without a name; S_FILESTATIC's file name with its
# offset in /names.
# An inline site's file change names a file by its offset in the module's
# file checksums, which differ between modules, and llvm-pdbutil doesn't
# resolve the name (it prints none, or a piece of another string). The
# file isn't compared here; turbonop-fold's tests compare it (by name and
# checksum) in the objects.
SETFILE = re.compile(r"setfile .*? 0x[0-9A-F]+(?= \||$)")
BARE = re.compile(r"(original type = )0x[0-9A-F]+|(file name = )\d+ (?=\()")


def dump(pdbutil, pdb):
    """llvm-pdbutil's dump of modules, symbols, lines, and inlinee lines,
    written to a file (it can be gigabytes) and read back line by line."""
    fd, path = tempfile.mkstemp(suffix=".txt")
    with os.fdopen(fd, "wb") as out:
        p = subprocess.run([pdbutil, "dump", "-modules", "-symbols", "-l", "-il", pdb],
                           stdout=out, stderr=subprocess.PIPE)
    if p.returncode != 0:
        raise SystemExit(f"{pdb}: llvm-pdbutil failed: {p.stderr.decode(errors='replace')}")
    return path


class Proc:
    __slots__ = ("module", "name", "section", "start", "size", "records", "lines", "sites")

    def __init__(self, module, name):
        self.module, self.name = module, name
        self.section, self.start, self.size = None, 0, 0
        self.records, self.lines, self.sites = [], [], []


def read(path):
    """Procedures, other records (by module), and each module's inlinee
    lines (id -> (line, file))."""
    procs, others, inlinees = [], [], collections.defaultdict(dict)
    part, module, cur, depth = None, None, None, 0
    line_blocks = []  # (module, section, start, file, entries)
    file_name = None
    index, streamless = None, set()
    pending = None  # (kind, text lines)

    def finish():
        nonlocal pending, cur, depth
        if pending is None:
            return
        kind, text = pending
        pending = None
        body = " ".join(t.strip() for t in text)
        if kind in PROC_KINDS and depth == 0:
            cur = Proc(module, body)
            m = re.search(r"addr = (\d{4}):(\d+), code size = (\d+)", body)
            if m:
                cur.section, cur.start, cur.size = int(m.group(1)), int(m.group(2)), int(m.group(3))
            procs.append(cur)
            cur.records.append((kind, text))
            depth = 1
            return
        if depth == 0:
            others.append((module, kind, text))
            return
        cur.records.append((kind, text))
        if kind in OPEN:
            depth += 1
        elif kind in CLOSE:
            depth -= 1
            if depth == 0:
                cur = None

    with open(path, encoding="utf-8", errors="replace") as f:
        for raw in f:
            line = raw.rstrip("\r\n")
            s = line.strip()
            if s in ("Symbols", "Lines", "Inlinee Lines", "Modules"):
                finish()
                part, module = s, None
                continue
            if s.startswith("====="):
                continue
            m = MOD.match(line)
            if m:
                finish()
                module = m.group(2)
                index = int(m.group(1))
                depth, cur = 0, None
                continue
            if part == "Modules" and s.startswith("debug stream: 65535"):
                # No debug stream: llvm-pdbutil prints the previous
                # module's lines again under this one.
                streamless.add(index)
                continue
            if part == "Symbols" and module is not None:
                m = RECORD.match(line)
                if m:
                    finish()
                    pending = (m.group(2), [m.group(3)])
                elif pending is not None and s:
                    pending[1].append(line)
            elif part == "Lines" and module is not None and s and index not in streamless:
                m = BLOCK.match(line)
                if m:
                    line_blocks.append([module, int(m.group(1), 16), int(m.group(2), 16),
                                        file_name, []])
                elif ENTRY.search(line) and line_blocks and not re.search(r"\(\S+: ", line) \
                        and not line.lstrip().startswith(("Col", "column")):
                    line_blocks[-1][4].extend(ENTRY.findall(line))
                else:
                    file_name = s
            elif part == "Inlinee Lines" and module is not None:
                m = INLINEE.match(line)
                if m:
                    inlinees[module][int(m.group(1), 16)] = (int(m.group(2)), m.group(3))
        finish()

    # Line blocks to the procedures whose code holds them.
    by_section = collections.defaultdict(list)
    for p in procs:
        if p.section is not None:
            by_section[p.section].append(p)
    for v in by_section.values():
        v.sort(key=lambda p: p.start)
    starts = {k: [p.start for p in v] for k, v in by_section.items()}
    import bisect
    orphan_lines = 0
    for module, section, start, file_name, entries in line_blocks:
        v = by_section.get(section, [])
        i = bisect.bisect_right(starts.get(section, []), start) - 1
        if i < 0 or not (v[i].start <= start < v[i].start + max(v[i].size, 1)):
            orphan_lines += 1
            continue
        p = v[i]
        rel = [(int(ln), int(a, 16) - p.start, bang) for ln, a, bang in entries]
        p.lines.append((file_name, start - p.start, tuple(rel)))
    return procs, others, inlinees, orphan_lines


def normalize(text, proc):
    """A record's text with indices, stream offsets, and addresses made
    comparable."""
    out = []
    for t in text:
        t = ANNOTATION.sub("", t.strip())
        t = INDEX.sub("(", t)
        t = LINKS.sub("", t)
        t = BARE.sub(lambda m: m.group(1) or m.group(2), t)
        t = SETFILE.sub("setfile", t)

        def addr(m):
            sec, off = int(m.group(1)), int(m.group(2))
            if proc is not None and sec == proc.section:
                return f"+{off - proc.start}"
            return "<addr>"
        t = ADDR.sub(addr, t)
        out.append(t)
    return " | ".join(out)


SITE = re.compile(r"inlinee = 0x([0-9A-F]+) \(")


def proc_key(p, inlinees):
    parts = []
    for kind, text in p.records:
        n = f"{kind} {normalize(text, p)}"
        if kind == "S_INLINEES":
            # A set of functions, in id order: the ids' order differs
            # between objects.
            head, _, rest = n.partition(" | ")
            n = head + " | " + " | ".join(sorted(rest.split(" | ")))
        if kind in ("S_INLINESITE", "S_INLINESITE2"):
            m = SITE.search(" ".join(text))
            if m:
                n += f" src={inlinees[p.module].get(int(m.group(1), 16))}"
        parts.append(n)
    for file_name, at, entries in sorted(p.lines):
        parts.append(f"lines {file_name} +{at} {entries}")
    return "\n".join(parts)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("original")
    ap.add_argument("candidate")
    ap.add_argument("--pdbutil", default=PDBUTIL)
    ap.add_argument("--show", type=int, default=5)
    ap.add_argument("--modules", default=r"turbolib\d+\.obj")
    a = ap.parse_args()
    synth = re.compile(a.modules)
    results = []
    wheres = []
    for pdb in (a.original, a.candidate):
        path = dump(a.pdbutil, pdb)
        try:
            procs, others, inlinees, orphans = read(path)
        finally:
            os.remove(path)
        keys = collections.Counter(proc_key(p, inlinees) for p in procs)
        other = collections.Counter(f"{k} {normalize(t, None)}" for _, k, t in others
                                    if k not in MODULE_KINDS | LAYOUT_KINDS)
        moved = sum(1 for p in procs if synth.search(p.module))
        lines = sum(len(p.lines) for p in procs)
        # Which modules each other record is in, for reports.
        where = collections.defaultdict(collections.Counter)
        for m, k, t in others:
            where[f"{k} {normalize(t, None)}"][os.path.basename(m)] += 1
        wheres.append(where)
        results.append((keys, other, moved, len(procs), lines, orphans))
        print(f"{pdb}: {len(procs)} procedures ({moved} in synthesized members), "
              f"{lines} line blocks ({orphans} outside procedures), "
              f"{sum(other.values())} other records")
    (k1, o1, _, _, _, _), (k2, o2, _, _, _, _) = results
    bad = 0
    for what, x, y in (("procedures", k1, k2), ("other records", o1, o2)):
        missing, extra = x - y, y - x
        if missing or extra:
            bad += 1
            print(f"{what}: {sum(missing.values())} only in the original, "
                  f"{sum(extra.values())} only in the candidate")
            for label, c in (("original", missing), ("candidate", extra)):
                for k in list(c)[:a.show]:
                    print(f"--- only in the {label}:\n{k[:3000]}")
            if what == "other records":
                # The modules whose counts of them differ.
                mods = collections.Counter()
                for k in list(missing) + list(extra):
                    d = wheres[0][k].copy()
                    d.subtract(wheres[1][k])
                    for m, n in d.items():
                        if n:
                            mods[m] += abs(n)
                print("in modules: " + ", ".join(f"{m} ({n})" for m, n in mods.most_common(20)))
        else:
            print(f"{what}: the same ({sum(x.values())})")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
