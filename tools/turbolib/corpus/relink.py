"""Links programs of a Ninja build twice, against the build's libraries and
against other copies of them, and compares the two programs.

    python relink.py <build dir> <libraries dir> <program>...
                     [--linker link|lld]... [--opt REF|NOREF]...
                     [--debug FULL|GHASH]... [--matrix]
                     [--compare images|bytes|both] [--turbolib PATH]
                     [--link LINK] [--lld LLD] [--keep DIR]

<program> is a target as build.ninja names it (bin\\llvm-nm.exe). Each is
linked as its build statement links it (the objects, flags, and
libraries), except:

- /Brepro and /INCREMENTAL:NO, so that the same inputs give the same bytes;
- the linker, /OPT and /DEBUG as chosen (both links of a configuration with
  the same ones);
- in the second link, each library lib\\X.lib is <libraries dir>\\X.lib
  where that file exists (as relib.py --keep leaves them, or a build with
  turbolib in lib.exe's place).

Each configuration is linked and compared on its own: link.exe and
lld-link (--linker; both by default), /OPT:REF and /OPT:NOREF (--opt; both
by default), and /DEBUG:FULL and, for lld-link, /DEBUG:GHASH (--debug; both
by default). --matrix is all of them, the default.

--compare images (the default) runs `turbolib compare-images` on the two
programs and their PDBs: equivalence by content, as COMDAT order may
change addresses. --compare bytes requires the two images to be byte for
byte the same, as phase 1's libraries (turbolib with --fold=none) must be;
the PDBs are then not compared, as link.exe's differ between two links of
the same inputs (measured with 14.52: module streams in another order, the
IPI stream another size, and different picks among same-named records).
--compare both does both.

A first run with <libraries dir> holding copies of the build's own
libraries checks this script, and compare-images: every configuration must
compare equal.
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
LLD = os.path.join(os.environ.get("LLVM_BIN", r"C:\jm\dev\llvm-project\build-release\bin"),
                   "lld-link.exe")
REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))
TURBOLIB = os.path.join(REPO, "target", "release", "turbolib.exe")


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


def chosen_here(flag):
    """Whether the build's flag is one this script sets."""
    f = flag.upper().lstrip("/-")
    return f.startswith(("INCREMENTAL", "OPT:REF", "OPT:NOREF", "DEBUG"))


def link_args(program, out, opt, debug):
    """The linker's arguments: the build statement's, for output `out`,
    with /OPT and /DEBUG as given."""
    v = program["vars"]
    flags = [f for f in words(v.get("LINK_FLAGS", "")) if not chosen_here(f)]
    ins = [p for p in program["in"] if not p.lower().endswith(".res")]
    ins += [p for p in program["in"] if p.lower().endswith(".res")]
    return (["/nologo"] + ins + [f"/out:{out}", f"/pdb:{out[:-4]}.pdb", "/version:0.0"]
            + flags + ["/INCREMENTAL:NO", "/Brepro", f"/OPT:{opt}", f"/DEBUG:{debug}"]
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


def link(linker, cwd, args):
    p = subprocess.run([linker] + args, cwd=cwd, capture_output=True)
    if p.returncode != 0:
        raise SystemExit(f"link failed ({p.returncode}): {p.stdout.decode(errors='replace')}"
                         f"{p.stderr.decode(errors='replace')}")


def read(path):
    with open(path, "rb") as f:
        return f.read()


def configurations(a):
    """(linker, /OPT, /DEBUG) for each configuration asked for."""
    out = []
    for linker in a.linker or ["link", "lld"]:
        for opt in a.opt or ["REF", "NOREF"]:
            for debug in a.debug or ["FULL", "GHASH"]:
                if debug == "GHASH" and linker != "lld":
                    continue
                out.append((linker, opt, debug))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("build_dir")
    ap.add_argument("libraries_dir")
    ap.add_argument("programs", nargs="+")
    ap.add_argument("--linker", action="append", choices=["link", "lld"])
    ap.add_argument("--opt", action="append", choices=["REF", "NOREF"], type=str.upper)
    ap.add_argument("--debug", action="append", choices=["FULL", "GHASH"], type=str.upper)
    ap.add_argument("--matrix", action="store_true", help="every configuration (the default)")
    ap.add_argument("--compare", choices=["images", "bytes", "both"], default="images")
    ap.add_argument("--turbolib", default=TURBOLIB)
    ap.add_argument("--link", default=LINK)
    ap.add_argument("--lld", default=LLD)
    ap.add_argument("--keep")
    a = ap.parse_args()
    if a.matrix:
        a.linker = a.opt = a.debug = None
    scratch = a.keep or tempfile.mkdtemp(prefix="relink-")
    # Both links run in one mirror of the build directory, with the same
    # arguments and output paths, so the PDBs' records of the directory,
    # command line, and paths agree; only the library files are swapped.
    mirror = os.path.join(scratch, "mirror")
    bad = 0
    for target in a.programs:
        program = read_program(a.build_dir, target)
        inputs = relative_inputs(program)
        out = target
        os.makedirs(os.path.join(mirror, os.path.dirname(out)), exist_ok=True)
        for linker, opt, debug in configurations(a):
            config = f"{linker} /OPT:{opt} /DEBUG:{debug}"
            tag = f"{linker}-{opt}-{debug}".lower()
            exe = a.link if linker == "link" else a.lld
            kept = {}
            for which in ("original", "candidate"):
                swapped = 0
                for rel in inputs:
                    src = os.path.join(a.build_dir, rel)
                    alt = os.path.join(a.libraries_dir, os.path.basename(rel))
                    if which == "candidate" and rel.lower().endswith(".lib") and os.path.exists(alt):
                        src = alt
                        swapped += 1
                    place(src, os.path.join(mirror, rel))
                if which == "candidate":
                    print(f"{target} [{config}]: {swapped} libraries swapped")
                for ext in (".exe", ".pdb"):
                    stale = os.path.join(mirror, out[:-4] + ext)
                    if os.path.exists(stale):
                        os.remove(stale)
                link(exe, mirror, link_args(program, out, opt, debug))
                for ext in (".exe", ".pdb"):
                    dest = os.path.join(scratch, f"{os.path.basename(out)[:-4]}.{tag}.{which}{ext}")
                    os.replace(os.path.join(mirror, out[:-4] + ext), dest)
                    kept[(which, ext)] = dest
            if a.compare in ("bytes", "both"):
                x, y = read(kept[("original", ".exe")]), read(kept[("candidate", ".exe")])
                same = x == y
                print(f"{target} [{config}]: image {'same' if same else 'DIFFERS'}"
                      f" ({len(x)} and {len(y)} bytes)")
                bad += not same
            if a.compare in ("images", "both"):
                p = subprocess.run([a.turbolib, "compare-images", "--stats",
                                    kept[("original", ".exe")], kept[("original", ".pdb")],
                                    kept[("candidate", ".exe")], kept[("candidate", ".pdb")]],
                                   capture_output=True, text=True)
                verdict = [line for line in p.stdout.splitlines()
                           if line.startswith(("equivalent", "NOT equivalent"))]
                print(f"{target} [{config}]: compare-images: "
                      f"{verdict[0] if verdict else 'no verdict'} ({p.stderr.strip()})")
                if p.returncode != 0:
                    bad += 1
                    print(p.stdout)
    if not a.keep and not bad:
        shutil.rmtree(scratch, ignore_errors=True)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
