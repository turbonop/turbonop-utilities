"""Links the phase 4 fixtures (testdata\\coff\\loose, from build.cmd) into
main.cpp's program, against a library lib.exe made and against the same
library folded by turbolib in each of phase 4's modes, with link.exe and
lld-link, under /OPT:REF and /OPT:NOREF, runs each program, and runs
`turbolib verify` on each folded library with the mode's options.

    python check.py <turbolib.exe> [--work DIR]

Every program must print what the original prints, but for the "choice:"
line (which copy of variant(), selected, and refers() a link keeps): with
--fold=select-any any copy may be kept, so each value on that line need
only be one some choice of copies gives.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
FIXTURES = os.path.join(ROOT, "testdata", "coff", "loose")
VS = r"C:\Program Files\Microsoft Visual Studio\18\Enterprise"
LLVM_BIN = os.environ.get("LLVM_BIN", r"C:\jm\dev\llvm-project\build-release\bin")
SETS = ["msvc-od", "msvc-odnogy", "msvc-o2", "clang-od"]
# turbolib's options, and verify's.
MODES = [
    ("--fold=identical", ""),
    ("--fold=select-any", "--fold=select-any"),
    ("--fold=select-any --pick=largest --pull=relaxed", "--fold=select-any --pull=relaxed"),
    ("--fold=identical --locals=copy", "--locals=copy"),
    ("--fold=identical --locals=merge --pull=relaxed", "--locals=merge --pull=relaxed"),
    ("--fold=select-any --locals=merge --precomp=library", "--fold=select-any --locals=merge"),
]


def vcvars():
    """The environment vcvars64.bat makes."""
    bat = os.path.join(VS, r"VC\Auxiliary\Build\vcvars64.bat")
    out = subprocess.run(f'"{bat}" >nul && set', shell=True, capture_output=True, text=True)
    env = {}
    for line in out.stdout.splitlines():
        k, eq, v = line.partition("=")
        if eq:
            env[k] = v
    return env


def run(args, cwd, env, log):
    """Runs a tool with its output in a file: link.exe's mspdbsrv keeps
    any pipe it inherits open."""
    with open(log, "ab") as f:
        f.write(("> " + " ".join(args) + "\n").encode())
        f.flush()
        p = subprocess.run(args, cwd=cwd, env=env, stdout=f, stderr=subprocess.STDOUT,
                           stdin=subprocess.DEVNULL)
    return p.returncode


def must(args, cwd, env, log):
    if run(args, cwd, env, log) != 0:
        raise SystemExit(f"failed: {' '.join(args)}; see {log}")


def same_output(original, folded, select_any):
    """Whether a folded program's output is acceptable."""
    if original[0] != folded[0]:
        return False
    a = original[1].splitlines()
    b = folded[1].splitlines()
    if len(a) != len(b):
        return False
    for x, y in zip(a, b):
        if x.startswith("choice:") and select_any:
            # Each object's sum of variant() (11 or 21), selected (1 or 2),
            # and refers() (1000 or 2000), from whichever copies the link
            # kept (or, at /O2, inlined).
            possible = {str(v + s + r) for v in (11, 21) for s in (1, 2) for r in (1000, 2000)}
            if any(v not in possible for v in y.split()[1:]):
                return False
        elif x != y:
            return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("turbolib")
    ap.add_argument("--work")
    a = ap.parse_args()
    a.turbolib = os.path.abspath(a.turbolib)
    work = a.work or tempfile.mkdtemp(prefix="turbolib-loose-")
    os.makedirs(work, exist_ok=True)
    log = os.path.join(work, "log.txt")
    env = vcvars()
    path = next(v for k, v in env.items() if k.upper() == "PATH")
    cl, lib_exe, link = (shutil.which(t, path=path) for t in ("cl.exe", "lib.exe", "link.exe"))
    turbolib = os.path.join(work, "lib.exe")
    shutil.copyfile(a.turbolib, turbolib)
    main_obj = os.path.join(work, "main.obj")
    must([cl, "/nologo", "/c", "/Od", "/Z7", "/GS-", os.path.join(HERE, "main.cpp"),
          f"/Fo:{main_obj}"], work, env, log)
    bad = 0
    runs = 0
    for name in SETS:
        objs = [os.path.join(FIXTURES, f"{name}-{m}.obj") for m in "abc"]
        must([lib_exe, "/nologo", f"/out:{name}-original.lib", *objs], work, env, log)
        for mi, (options, verify) in enumerate(MODES):
            folded = f"{name}-{mi}.lib"
            fold_env = dict(env, TURBOLIB_OPTIONS=options + " --stats")
            must([turbolib, "/nologo", f"/out:{folded}", *objs], work, fold_env, log)
            v = run([a.turbolib, "verify", *verify.split(), f"{name}-original.lib", folded],
                    work, env, log)
            if v != 0:
                print(f"{name}, {options}: verify FAILED")
                bad += 1
            for linker in ("link", "lld-link"):
                exe = link if linker == "link" else os.path.join(LLVM_BIN, "lld-link.exe")
                for opt in ("REF", "NOREF"):
                    outputs = {}
                    for which, lib in (("original", f"{name}-original.lib"), ("folded", folded)):
                        out = f"{name}-{mi}-{linker}-{opt}-{which}"
                        must([exe, "/nologo", "/DEBUG", "/INCREMENTAL:NO", f"/OPT:{opt}",
                              f"/OUT:{out}.exe", f"/PDB:{out}.pdb", main_obj, lib],
                             work, env, log)
                        p = subprocess.run([os.path.join(work, out + ".exe")],
                                           capture_output=True, text=True)
                        outputs[which] = (p.returncode, p.stdout)
                    ok = same_output(outputs["original"], outputs["folded"],
                                     "select-any" in options)
                    runs += 1
                    if not ok:
                        bad += 1
                        print(f"{name}, {options}, {linker} /OPT:{opt}: output DIFFERS")
                        print("  original:", outputs["original"])
                        print("  folded:  ", outputs["folded"])
        print(f"{name}: {len(MODES)} modes checked")
    print(f"{runs} program pairs, {bad} failures; work directory: {work}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
