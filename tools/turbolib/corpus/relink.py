"""Links programs of a Ninja build twice, against the build's libraries and
against other copies of them, and compares the two programs and PDBs.

    python relink.py <build dir> <libraries dir> <program>... [--link LINK]
                     [--keep DIR]

<program> is a target as build.ninja names it (bin\\llvm-nm.exe). Each is
linked with link.exe, as its build statement links it (the objects, flags,
and libraries), except:

- /Brepro and /INCREMENTAL:NO, so that the same inputs give the same bytes;
- in the second link, each library lib\\X.lib is <libraries dir>\\X.lib
  where that file exists (as relib.py --keep leaves them, or a build with
  turbolib in lib.exe's place).

The two images must be byte for byte the same. The PDBs are reported, not
compared: link.exe's PDBs differ between two links of the same inputs, even
with /Brepro and /THREADS:1 (measured with 14.52: module streams in another
order, the IPI stream another size, and different picks among same-named
records), so their equivalence needs a canonical comparison.

A first run with <libraries dir> holding copies of the build's own libraries
checks this script: the images must match.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import relib  # noqa: E402

LINK = (r"C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Tools\MSVC"
        r"\14.52.37025\bin\Hostx64\x64\link.exe")


def read_program(build_dir, target):
    cur = None
    for line in relib.ninja_lines(os.path.join(build_dir, "build.ninja")):
        m = re.match(r"build (.+?): (\S+)(.*)", line)
        if m:
            if cur:
                return cur
            outputs = relib.split_paths(m.group(1))
            if outputs[0] == target and "EXECUTABLE_LINKER" in m.group(2):
                explicit = m.group(3).split(" || ")[0].split(" | ")[0]
                cur = {"in": relib.split_paths(explicit), "vars": {}}
            continue
        mm = re.match(r"\s+(\w+) = (.*)", line)
        if mm and cur is not None:
            cur["vars"][mm.group(1)] = mm.group(2)
    if cur:
        return cur
    raise SystemExit(f"{target}: no executable build statement")


def words(text):
    """A Ninja variable's command-line words: spaces separate them, and
    double quotes group a path with spaces (and are dropped)."""
    out, cur, quoted, any_ = [], "", False, False
    for c in relib.unescape(text):
        if c == '"':
            quoted, any_ = not quoted, True
        elif c == " " and not quoted:
            if any_:
                out.append(cur)
            cur, any_ = "", False
        else:
            cur, any_ = cur + c, True
    if any_:
        out.append(cur)
    return out


def link_args(program, out):
    """link.exe's arguments: the build statement's, for output `out`."""
    v = program["vars"]
    flags = [f for f in words(v.get("LINK_FLAGS", ""))
             if not f.upper().startswith("/INCREMENTAL")]
    ins = [p for p in program["in"] if not p.lower().endswith(".res")]
    ins += [p for p in program["in"] if p.lower().endswith(".res")]
    return (["/nologo"] + ins + [f"/out:{out}", f"/pdb:{out[:-4]}.pdb", "/version:0.0"]
            + flags + ["/INCREMENTAL:NO", "/Brepro"]
            + words(v.get("LINK_PATH", "")) + words(v.get("LINK_LIBRARIES", "")))


def relative_inputs(program):
    """The relative paths a link reads, which the mirror must hold."""
    v = program["vars"]
    paths = list(program["in"]) + words(v.get("LINK_LIBRARIES", ""))
    paths += [f for f in words(v.get("LINK_FLAGS", "")) if not f.startswith(("/", "-"))]
    return [p for p in paths if not os.path.isabs(p) and os.sep in p.replace("/", os.sep)]


def place(src, dst):
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    if os.path.exists(dst):
        os.remove(dst)
    try:
        os.link(src, dst)
    except OSError:
        shutil.copyfile(src, dst)


def link(link_exe, cwd, program, out):
    p = subprocess.run([link_exe] + link_args(program, out), cwd=cwd, capture_output=True)
    if p.returncode != 0:
        raise SystemExit(f"link failed ({p.returncode}): {p.stdout.decode(errors='replace')}")


def read(path):
    with open(path, "rb") as f:
        return f.read()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("build_dir")
    ap.add_argument("libraries_dir")
    ap.add_argument("programs", nargs="+")
    ap.add_argument("--link", default=LINK)
    ap.add_argument("--keep")
    a = ap.parse_args()
    scratch = a.keep or tempfile.mkdtemp(prefix="relink-")
    # Both links run in one mirror of the build directory, with the same
    # arguments and output paths, so the PDBs' records of the directory,
    # command line, and paths agree; only the library files are swapped.
    mirror = os.path.join(scratch, "mirror")
    bad = 0
    for target in a.programs:
        program = read_program(a.build_dir, target)
        inputs = relative_inputs(program)
        for rel in inputs:
            place(os.path.join(a.build_dir, rel), os.path.join(mirror, rel))
        out = target
        os.makedirs(os.path.join(mirror, os.path.dirname(out)), exist_ok=True)
        results = {}
        for which in ("original", "candidate"):
            if which == "candidate":
                swapped = 0
                for rel in inputs:
                    alt = os.path.join(a.libraries_dir, os.path.basename(rel))
                    if rel.lower().endswith(".lib") and os.path.exists(alt):
                        place(alt, os.path.join(mirror, rel))
                        swapped += 1
                print(f"{target}: {swapped} libraries swapped")
            link(a.link, mirror, program, out)
            for ext in (".exe", ".pdb"):
                path = os.path.join(mirror, out[:-4] + ext)
                results[(which, ext)] = read(path)
                os.replace(path, os.path.join(scratch, f"{os.path.basename(out)[:-4]}.{which}{ext}"))
        x, y = results[("original", ".exe")], results[("candidate", ".exe")]
        same = x == y
        print(f"{target}: image {'same' if same else 'DIFFERS'} ({len(x)} and {len(y)} bytes)")
        bad += not same
        x, y = results[("original", ".pdb")], results[("candidate", ".pdb")]
        print(f"{target}: PDBs {len(x)} and {len(y)} bytes (not compared)")
    if not a.keep and not bad:
        shutil.rmtree(scratch, ignore_errors=True)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
