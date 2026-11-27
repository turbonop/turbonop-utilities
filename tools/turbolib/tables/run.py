"""Links programs against a library and its folded copy, and compares the
images' side tables (phase 2.3 of docs/turbolib.md).

For each configuration (an architecture, a compiler, and its options), this
compiles tools/coff/tables/one.cpp and two.cpp and three.cpp here (which
share shared.h's inline functions) into orig.lib with lib.exe, and folds
them into fold.lib with turbolib's lib personality
(TURBOLIB_OPTIONS=--fold=identical). Then main.cpp is linked against each
with link.exe and lld-link, under /OPT:NOREF and /OPT:REF, and:

- the load config's tables (GuardFidTable, GuardIatTable, GuardLJmpTable,
  GuardEHContTable, SEHTable) are read with llvm-readobj, the dynamic
  relocations (retpoline) with dumpbin, the volatile metadata from the
  image, and the PDB's FPO data with llvm-pdbutil;
- each address is named by the map file (function+offset), and each table's
  names must be the same for both libraries;
- the x64 and x86 programs run, and must print the same and succeed.

    python run.py [config...]

Outputs land in out/ (which .gitignore keeps out). TURBOLIB names
turbolib.exe (the default is target/release's); LLVM_BIN, the LLVM tools.
"""

import concurrent.futures
import os
import re
import shutil
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
SRC = os.path.join(ROOT, "tools", "coff", "tables")
OUT = os.path.join(HERE, "out")
VS = r"C:\Program Files\Microsoft Visual Studio\18\Enterprise"
LLVM_BIN = os.environ.get("LLVM_BIN", r"C:\jm\dev\llvm-project\build-release\bin")
TURBOLIB = os.environ.get("TURBOLIB", os.path.join(ROOT, "target", "release", "turbolib.exe"))
VCVARS = {
    "x64": "vcvars64.bat",
    "x86": "vcvarsamd64_x86.bat",
    "arm64": "vcvarsamd64_arm64.bat",
}
CLANG = os.path.join(LLVM_BIN, "clang-cl.exe")

# name: (architecture, compiler, compiler options, linker options)
CONFIGS = {
    "x64-cf": ("x64", "cl", "/Gy /guard:cf /guard:ehcont", "/guard:cf /guard:ehcont /CETCOMPAT"),
    "x64-cf-nogy": ("x64", "cl", "/Gy- /guard:cf /guard:ehcont", "/guard:cf /guard:ehcont /CETCOMPAT"),
    "x64-clang": ("x64", CLANG, "/guard:cf /guard:ehcont", "/guard:cf /guard:ehcont /CETCOMPAT"),
    "x86": ("x86", "cl", "/Gy /guard:cf", "/guard:cf /safeseh"),
    "x86-nogy": ("x86", "cl", "/Gy- /guard:cf", "/guard:cf /safeseh"),
    "x86-clang": ("x86", CLANG + " -m32", "/guard:cf", "/guard:cf /safeseh"),
    "x64-voltbl": ("x64", "cl", "/Gy /volatileMetadata", ""),
    "x64-voltbl-nogy": ("x64", "cl", "/Gy- /volatileMetadata", ""),
    "x64-retpoline": ("x64", "cl", "/Gy /d2guardretpoline", "/guard:retpoline"),
    "arm64-cf": ("arm64", "cl", "/Gy /guard:cf /guard:ehcont", "/guard:cf /guard:ehcont"),
}

_envs = {}


def env(arch):
    if arch not in _envs:
        bat = os.path.join(VS, "VC", "Auxiliary", "Build", VCVARS[arch])
        out = subprocess.run(f'cmd /c ""{bat}" >nul && set"', capture_output=True, text=True, shell=True).stdout
        e = dict(line.split("=", 1) for line in out.splitlines() if "=" in line)
        _envs[arch] = e
    return _envs[arch]


def run(cmd, cwd, e, check=True):
    p = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True, shell=True)
    if check and p.returncode != 0:
        raise RuntimeError(f"{cmd}: {p.returncode}\n{p.stdout}\n{p.stderr}")
    return p


def symbols(map_path):
    """(va, name) from a map file's publics and statics, sorted."""
    out = []
    for line in open(map_path, errors="replace"):
        m = re.match(r"\s*([0-9a-fA-F]{4}):([0-9a-fA-F]{8})\s+(\S+)\s+([0-9a-fA-F]{8,16})\s", line)
        # Labels' names differ between copies of the same code; absolute
        # symbols, and the statics of COMDAT copies the link discarded
        # (which link.exe lists at offsets past 0x80000000), name nothing.
        if m and not m.group(3).startswith("$") and m.group(1) != "0000" and int(m.group(2), 16) < 0x80000000:
            out.append((int(m.group(4), 16), m.group(3)))
    out.sort()
    return out


def namer(syms):
    import bisect

    vas = [v for v, _ in syms]

    def name(va):
        i = bisect.bisect_right(vas, va) - 1
        if i < 0:
            return "(before the first section)"
        return syms[i][1] if va == vas[i] else f"{syms[i][1]}+{va - vas[i]:#x}"

    return name


def image(path):
    d = open(path, "rb").read()
    pe = struct.unpack_from("<I", d, 0x3C)[0]
    nsec, opt = struct.unpack_from("<H", d, pe + 6)[0], struct.unpack_from("<H", d, pe + 20)[0]
    magic = struct.unpack_from("<H", d, pe + 24)[0]
    base = struct.unpack_from("<Q", d, pe + 48)[0] if magic == 0x20B else struct.unpack_from("<I", d, pe + 52)[0]
    secs = []
    for i in range(nsec):
        o = pe + 24 + opt + 40 * i
        vsize, va, rsize, ptr = struct.unpack_from("<IIII", d, o + 8)
        secs.append((va, max(vsize, rsize), ptr))

    def at(rva, n):
        for va, size, ptr in secs:
            if va <= rva < va + size:
                return d[ptr + rva - va: ptr + rva - va + n]
        return b""

    return base, at


def tables(exe, pdb, e, name):
    """Each table's entries, named."""
    out = {}
    text = run(f'"{LLVM_BIN}\\llvm-readobj.exe" --coff-load-config "{exe}"', None, e).stdout
    for t in ("GuardFidTable", "GuardIatTable", "GuardLJmpTable", "GuardEHContTable", "SEHTable"):
        m = re.search(t + r" \[\n(.*?)\n\]", text, re.S)
        if m:
            vals = [int(l.split()[0], 16) for l in m.group(1).splitlines() if l.strip()]
            out[t] = sorted(name(v) for v in vals)
    base, at = image(exe)
    m = re.search(r"VolatileMetadataPointer: (0x[0-9A-F]+)", text)
    if m and int(m.group(1), 16):
        v = struct.unpack("<6I", at(int(m.group(1), 16) - base, 24))
        table, size = v[2], v[3]
        rvas = struct.unpack(f"<{size // 4}I", at(table, size)) if size else ()
        out["VolatileAccess"] = sorted(name(base + r) for r in rvas)
    dump = run(f'dumpbin /nologo /loadconfig "{exe}"', None, e).stdout
    kind = None
    dvrt = []
    for line in dump.splitlines():
        m = re.search(r"Symbol VA: \S+ (\S+)", line)
        if m:
            kind = m.group(1)
            continue
        m = re.search(r"= page \S+ rva ([0-9A-F]+)(.*)", line)
        if m and kind:
            dvrt.append(f"{kind} {name(base + int(m.group(1), 16))}{m.group(2)}")
    if dvrt:
        out["DynamicRelocations"] = sorted(dvrt)
    if pdb and os.path.exists(pdb):
        fpo = run(f'"{LLVM_BIN}\\llvm-pdbutil.exe" dump -fpo "{pdb}"', None, e, check=False).stdout
        rows = []
        for line in fpo.splitlines():
            m = re.match(r"\s*([0-9A-F]{8}) \|(.*)", line)
            if m:
                rows.append(name(base + int(m.group(1), 16)) + " |" + re.sub(r"\s+", " ", m.group(2)))
        if rows:
            out["FPO"] = sorted(rows)
    return out


def config(cfg):
    arch, cc, cflags, lflags = CONFIGS[cfg]
    e = dict(env(arch))
    d = os.path.join(OUT, cfg)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    report = [f"== {cfg}"]
    for src in ("one.cpp", "two.cpp"):
        run(f'{cc} /nologo /c /O2 /EHsc {cflags} "{os.path.join(SRC, src)}"', d, e)
    for src in ("three.cpp", "main.cpp"):
        run(f'{cc} /nologo /c /O2 /EHsc {cflags} "{os.path.join(HERE, src)}"', d, e)
    run("lib /nologo /out:orig.lib one.obj two.obj three.obj", d, e)
    shutil.copy(TURBOLIB, os.path.join(d, "lib.exe"))
    fe = dict(e, TURBOLIB_OPTIONS="--fold=identical --stats")
    stats = run(r".\lib.exe /nologo /out:fold.lib one.obj two.obj three.obj", d, fe)
    report.append("  turbolib: " + " ".join((stats.stdout + stats.stderr).split())[:400])
    runnable = arch in ("x64", "x86")
    ok = True
    for linker in ("link", "lld"):
        for opt in ("noref", "ref"):
            got = {}
            for lib in ("orig", "fold"):
                stem = f"{lib}-{linker}-{opt}"
                common = f"/debug /out:{stem}.exe /map:{stem}.map /opt:{opt} main.obj {lib}.lib kernel32.lib {lflags}"
                if opt == "ref":
                    common += " /opt:noicf"
                if linker == "link":
                    p = run(f"link /nologo /incremental:no {common}", d, e, check=False)
                else:
                    # lld-link writes the longjmp table only when asked.
                    common = common.replace("/guard:cf", "/guard:cf,longjmp")
                    # and has no /guard:retpoline.
                    common = common.replace("/guard:retpoline", "")
                    p = run(f'"{LLVM_BIN}\\lld-link.exe" {common}', d, e, check=False)
                if p.returncode != 0:
                    report.append(f"  {stem}: link failed: {(p.stdout + p.stderr)[-300:]}")
                    ok = False
                    continue
                t = tables(os.path.join(d, stem + ".exe"), os.path.join(d, stem + ".pdb"), e,
                           namer(symbols(os.path.join(d, stem + ".map"))))
                if runnable:
                    try:
                        r = subprocess.run([os.path.join(d, stem + ".exe")], capture_output=True, text=True)
                        t["run"] = [f"exit {r.returncode & 0xFFFFFFFF:#x}: {r.stdout.strip()}"]
                    except OSError as x:
                        t["run"] = [f"didn't start: {x}"]
                got[lib] = t
            if len(got) < 2:
                continue
            o, f = got["orig"], got["fold"]
            junk = "(before the first section)"
            for k in sorted((set(o) | set(f)) - {"run"}):
                # link.exe gives entries naming the labels of COMDAT copies
                # it discarded (which clang-cl's per-object tables name)
                # addresses before the first section; folding leaves no
                # copies to discard. Those aren't compared.
                a = [x for x in o.get(k, []) if x != junk]
                b = [x for x in f.get(k, []) if x != junk]
                same = a == b
                ok &= same
                extra = len(o.get(k, [])) - len(a), len(f.get(k, [])) - len(b)
                note = f" (and {extra[0]}/{extra[1]} before the first section)" if any(extra) else ""
                report.append(f"  {linker} {opt} {k}: {len(a)} entries{note}" + ("" if same else "  DIFFERENT"))
                if not same:
                    report.append(f"    only orig: {sorted(set(a) - set(b))[:10]}")
                    report.append(f"    only fold: {sorted(set(b) - set(a))[:10]}")
            if runnable:
                r0, r1 = o["run"][0], f["run"][0]
                # The folded program must succeed, and print what the
                # original prints when the original succeeds.
                good = r1.startswith("exit 0x0:") and (r0 == r1 or not r0.startswith("exit 0x0:"))
                ok &= good
                report.append(f"  {linker} {opt} run: {r0} / {r1}" + ("" if good else "  DIFFERENT"))
    report.append(f"  {'OK' if ok else 'FAILED'}")
    return "\n".join(report), ok


def main():
    names = sys.argv[1:] or list(CONFIGS)
    for arch in {CONFIGS[n][0] for n in names}:
        env(arch)
    with concurrent.futures.ThreadPoolExecutor(os.cpu_count()) as pool:
        results = list(pool.map(config, names))
    for text, _ in results:
        print(text)
    sys.exit(0 if all(ok for _, ok in results) else 1)


if __name__ == "__main__":
    main()
