"""Links the debug-information fixtures (testdata\\coff\\fold, from build.cmd)
into main.cpp's program, against a library lib.exe made and against the
same library folded by turbolib, with link.exe and with lld-link, runs
each program, and compares the PDBs with ..\\corpus\\pdbcompare.py.

    python check.py <turbolib.exe> [--work DIR]

The libraries: MSVC's objects (/Od, /O2 with inline sites, and a PCH with
its users), clang-cl's (/Od, and /O2 with inline sites), and the PCH users
alone (their PCH object linked beside the library). Both
programs' output must be the same, and the PDBs the same apart from the
modules that hold the folded functions. The original is linked twice, as
a control: link.exe's PDBs differ between two links of the same inputs (in
which of the same-named S_UDT records it keeps, measured), so a difference
in records outside procedures is reported, and failing only for
procedures.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
FIXTURES = os.path.join(ROOT, "testdata", "coff", "fold")
VS = r"C:\Program Files\Microsoft Visual Studio\18\Enterprise"
LLVM_BIN = os.environ.get("LLVM_BIN", r"C:\jm\dev\llvm-project\build-release\bin")
# Each: the library's objects, main.cpp's defines, and objects linked
# beside the library.
SETS = {
    "msvc": (["msvc-a", "msvc-b", "msvc-c", "msvc-d", "msvc-pch", "msvc-u1", "msvc-u2"], [], []),
    "clang": (["clang-a", "clang-b", "clang-c", "clang-d"], ["/DNO_PCH"], []),
    # PCH users whose PCH object is not in their library.
    "users": (["msvc-u1", "msvc-u2"], ["/DONLY_PCH"], ["msvc-pch"]),
}


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
    if p.returncode != 0:
        raise SystemExit(f"failed ({p.returncode}): {' '.join(args)}; see {log}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("turbolib")
    ap.add_argument("--work")
    a = ap.parse_args()
    work = a.work or tempfile.mkdtemp(prefix="turbolib-debug-")
    os.makedirs(work, exist_ok=True)
    log = os.path.join(work, "log.txt")
    env = vcvars()
    path = next(v for k, v in env.items() if k.upper() == "PATH")
    cl, lib_exe, link = (shutil.which(t, path=path) for t in ("cl.exe", "lib.exe", "link.exe"))
    turbolib = os.path.join(work, "lib.exe")
    shutil.copyfile(a.turbolib, turbolib)
    bad = 0
    for name, (objects, defines, beside) in SETS.items():
        objs = [os.path.join(FIXTURES, o + ".obj") for o in objects]
        beside = [os.path.join(FIXTURES, o + ".obj") for o in beside]
        main_obj = os.path.join(work, f"main-{name}.obj")
        run([cl, "/nologo", "/c", "/Od", "/Z7", "/GS-", *defines,
             os.path.join(HERE, "main.cpp"), f"/Fo:{main_obj}"], work, env, log)
        run([lib_exe, "/nologo", f"/out:{name}-original.lib", *objs], work, env, log)
        fold_env = dict(env, TURBOLIB_OPTIONS="--fold=identical --stats")
        run([turbolib, "/nologo", f"/out:{name}-folded.lib", *objs], work, fold_env, log)
        for linker in ("link", "lld-link"):
            exe = link if linker == "link" else os.path.join(LLVM_BIN, "lld-link.exe")
            outputs = {}
            for which in ("original", "again", "folded"):
                out = f"{name}-{linker}-{which}"
                lib = f"{name}-{'folded' if which == 'folded' else 'original'}.lib"
                run([exe, "/nologo", "/DEBUG", "/INCREMENTAL:NO", f"/OUT:{out}.exe",
                     f"/PDB:{out}.pdb", main_obj, *beside, lib], work, env, log)
                p = subprocess.run([os.path.join(work, out + ".exe")], capture_output=True,
                                   text=True)
                outputs[which] = (p.returncode, p.stdout)
            same = outputs["original"] == outputs["folded"]
            print(f"{name}, {linker}: output {'same' if same else 'DIFFERS'}: "
                  f"{outputs['folded'][1].strip()}")
            bad += not same
            # The original linked again is the control: link.exe's PDBs
            # differ between two links of the same inputs.
            for which in ("again", "folded"):
                p = subprocess.run(
                    [sys.executable, os.path.join(HERE, "..", "corpus", "pdbcompare.py"),
                     os.path.join(work, f"{name}-{linker}-original.pdb"),
                     os.path.join(work, f"{name}-{linker}-{which}.pdb")],
                    capture_output=True, text=True)
                print(f"{name}, {linker}, original and {which}:")
                print(p.stdout.rstrip())
                if which == "folded":
                    bad += p.returncode != 0 and "procedures: the same" not in p.stdout
    print(f"work directory: {work}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
