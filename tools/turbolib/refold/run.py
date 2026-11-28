"""Folds libraries that turbolib folded before, and checks the results:
the case of a build that makes libraries of libraries (Buck2).

    python run.py <turbolib.exe> [--work DIR] [--keep] [--inputs-from OLD]

For each compiler set (cl /Od, cl /O2, clang-cl /Od; all /Gy /Z7), it
compiles ../loose's a.cpp, b.cpp, and c.cpp, this directory's d.cpp and
e.cpp, and main.cpp, and makes, with each of turbolib's modes:

    first   = fold(a, b, c, d, e) a first fold, for comparison
    x1 = fold(a, b, c)            x2 = fold(d, e)
    alone  = fold(x1)            a folded library folded again by itself
    more    = fold(x1, d, e)      a folded library with more objects
    combine = fold(x1, x2)        two folded libraries' synthesized members merged
    again   = fold(combine)       folded three times

With --inputs-from, x1 and x2 are made by another turbolib.exe (an older
one: a library folded in the old form, each alias's default the absolute
`__turbolib$alias` every synthesized member defined), and the rest by
<turbolib.exe>, as a build that keeps libraries an older turbolib made
would. (The case that links x2 links it, against the fold, folded again,
in the new form: lld-link refuses a link of old-form and new-form
libraries of the same code, with duplicate symbols.)

lib.exe, with /WX, archives each library again by itself, and makes the
cases' lib.exe libraries (of folded libraries) with /WX too, unless they
are of --inputs-from's: a warning (LNK4006 for a name two members define)
is an error. llvm-lib combines x1 and x2 without a warning.

Each is made with --strict, so a fold that the self-check rejects (or that
fails otherwise) is an error, not the unfolded library; it must have no
two members of one name (lib.exe would replace one with the other when it
archives the library again); then `turbolib verify` checks it against
lib.exe's library of the same inputs. main.obj is linked against it (and
x2, for alone) and against lib.exe's library of the same inputs (and x2),
with link.exe and lld-link, under /OPT:REF and /OPT:NOREF:
`turbolib compare-images` must find the two programs equivalent but for
what it also finds comparing first's programs (the same set, mode, linker,
and /OPT), and each must print what main.obj linked against lib.exe's
library of the five objects prints. It prints each library's size against lib.exe's of the
same inputs, and how many copies folding removed.

Every wait has a timeout. The work directory (a temporary one unless
--work) is deleted at the end unless --keep.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
LOOSE = os.path.join(HERE, "..", "loose")
VS = r"C:\Program Files\Microsoft Visual Studio\18\Enterprise"
LLVM_BIN = os.environ.get("LLVM_BIN", r"C:\jm\dev\llvm-project\build-release\bin")
TIMEOUT = 600

COMMON = ["/nologo", "/c", "/std:c++17", "/EHsc", "/GS-", "/Brepro", "/Z7", "/Gy"]
SETS = {
    "msvc-od": ("cl", ["/Od", "/Ob0"]),
    "msvc-o2": ("cl", ["/O2"]),
    "clang-od": ("clang-cl", ["/Od", "-fno-addrsig"]),
}
MODES = {
    "types": "--fold=identical --precomp=library",
    "plain": "--fold=identical",
}
SOURCES = [(os.path.join(LOOSE, f"{m}.cpp"), m) for m in "abc"] + [
    (os.path.join(HERE, f"{m}.cpp"), m) for m in "de"
]


def vcvars():
    bat = os.path.join(VS, r"VC\Auxiliary\Build\vcvars64.bat")
    out = subprocess.run(f'"{bat}" >nul && set', shell=True, capture_output=True, text=True,
                         timeout=TIMEOUT)
    env = {}
    for line in out.stdout.splitlines():
        k, eq, v = line.partition("=")
        if eq:
            env[k] = v
    return env


class Runner:
    def __init__(self, work, env):
        self.work, self.env = work, env
        self.log = os.path.join(work, "log.txt")

    def run(self, args, env=None, cwd=None):
        """Runs a tool with its output in the log (link.exe's mspdbsrv keeps
        any pipe it inherits open); returns its exit code and output. A
        process that couldn't start (STATUS_DLL_INIT_FAILED, which a
        loaded machine gives now and then) is run once more."""
        out = os.path.join(self.work, "last.txt")
        for _ in range(2):
            with open(out, "wb") as f:
                p = subprocess.run(args, cwd=cwd or self.work, env=env or self.env, stdout=f,
                                   stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                                   timeout=TIMEOUT)
            if p.returncode != 0xC0000142:
                break
        text = open(out, "rb").read().decode("utf-8", "replace")
        with open(self.log, "a", encoding="utf-8") as f:
            f.write("> " + " ".join(args) + "\n" + text)
        return p.returncode, text

    def must(self, args, env=None, cwd=None):
        code, text = self.run(args, env, cwd)
        if code != 0:
            raise SystemExit(f"failed ({code}): {' '.join(args)}\n{text}")
        return text


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("turbolib")
    ap.add_argument("--work")
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("--inputs-from", help="the turbolib.exe that makes x1 and x2")
    a = ap.parse_args()
    turbolib_exe = os.path.abspath(a.turbolib)
    work = os.path.abspath(a.work or tempfile.mkdtemp(prefix="turbolib-refold-"))
    os.makedirs(work, exist_ok=True)
    env = vcvars()
    path = next(v for k, v in env.items() if k.upper() == "PATH")
    tools = {t: shutil.which(t + ".exe", path=path) for t in ("cl", "lib", "link")}
    tools["clang-cl"] = os.path.join(LLVM_BIN, "clang-cl.exe")
    tools["lld-link"] = os.path.join(LLVM_BIN, "lld-link.exe")
    lib_exe = tools["lib"]
    # turbolib's lib personality: a copy named lib.exe.
    turbolib = os.path.join(work, "lib.exe")
    shutil.copyfile(turbolib_exe, turbolib)
    # The turbolib that makes x1 and x2: the same, or --inputs-from's.
    inputs_tool = turbolib
    if a.inputs_from:
        os.makedirs(os.path.join(work, "inputs-from"), exist_ok=True)
        inputs_tool = os.path.join(work, "inputs-from", "lib.exe")
        shutil.copyfile(os.path.abspath(a.inputs_from), inputs_tool)
    tools["llvm-lib"] = os.path.join(LLVM_BIN, "llvm-lib.exe")
    r = Runner(work, env)
    bad = 0
    rows = []
    # What compare-images finds comparing a first fold's programs, by set,
    # mode, linker, and /OPT.
    baseline = {}
    for set_name, (compiler, flags) in SETS.items():
        d = os.path.join(work, set_name)
        os.makedirs(d, exist_ok=True)
        objs = {}
        for src, m in SOURCES + [(os.path.join(HERE, "main.cpp"), "main")]:
            obj = os.path.join(d, f"{m}.obj")
            r.must([tools[compiler], *COMMON, *flags, src, f"/Fo{obj}"])
            objs[m] = obj
        everything = os.path.join(d, "original.lib")
        r.must([lib_exe, "/nologo", "/Brepro", f"/out:{everything}",
                *(objs[m] for m in "abcde")])
        for mode, options in MODES.items():
            fold_env = dict(env, TURBOLIB_OPTIONS=options + " --strict --stats")

            def lib(out, inputs, tool, wx=False):
                path = os.path.join(d, f"{mode}-{out}.lib")
                text = r.must([tool, "/nologo", "/Brepro", *(["/WX"] if wx else []),
                               f"/out:{path}", *inputs],
                              env=fold_env if tool in (turbolib, inputs_tool) else env)
                return path, text

            def quiet(what, args):
                """A tool that must succeed without a warning."""
                nonlocal bad
                code, text = r.run(args)
                if code != 0 or "warning" in text.lower():
                    print(f"{set_name} {mode} {what}: FAILED ({code})\n{text}")
                    bad += 1

            x1, _ = lib("x1", [objs[m] for m in "abc"], inputs_tool)
            x2, _ = lib("x2", [objs[m] for m in "de"], inputs_tool)
            x2_new = lib("x2-new", [x2], turbolib)[0] if a.inputs_from else None
            quiet("llvm-lib x1 x2", [tools["llvm-lib"], "/WX",
                                     f"/out:{os.path.join(d, mode + '-llvm-lib.lib')}", x1, x2])
            cases = {
                "first": [objs[m] for m in "abcde"],
                "alone": [x1],
                "more": [x1, objs["d"], objs["e"]],
                "combine": [x1, x2],
            }
            made = {}
            for case, inputs in list(cases.items()) + [("again", None)]:
                if inputs is None:
                    inputs = [made["combine"]]
                # (lib.exe warns LNK4006 for the old form's alias default.)
                plain, _ = lib(f"{case}-plain", inputs, lib_exe, wx=not a.inputs_from)
                folded, text = lib(case, inputs, turbolib)
                quiet(f"{case}: lib.exe /WX archiving it again",
                      [lib_exe, "/nologo", "/Brepro", "/WX",
                       f"/out:{os.path.join(d, mode + '-' + case + '-relib.lib')}", folded])
                made[case] = folded
                m = re.search(r"(\d+) groups folded, (\d+) copies removed", text)
                removed = m.group(2) if m else "0"
                if "written unfolded" in text:
                    print(f"{set_name} {mode} {case}: written UNFOLDED")
                    bad += 1
                listed = r.must([turbolib, "/nologo", "/LIST", folded]).split()
                lower = [n.lower() for n in listed]
                if len(set(lower)) != len(lower):
                    print(f"{set_name} {mode} {case}: two members of one name: {sorted(listed)}")
                    bad += 1
                code, vtext = r.run([turbolib_exe, "verify", plain, folded])
                if code != 0:
                    print(f"{set_name} {mode} {case}: verify FAILED\n{vtext}")
                    bad += 1
                rows.append((set_name, mode, case, os.path.getsize(plain),
                             os.path.getsize(folded), removed))
                # (A case without d and e links them from x2. With
                # --inputs-from, the folded library links x2 folded again,
                # in the new form, and lib.exe's library x2 as it is: a link
                # of old-form and new-form libraries of the same code fails
                # with lld-link, duplicate symbols.)
                extra = {"original": [] if case != "alone" else [x2],
                         "folded": [] if case != "alone" else [x2_new or x2],
                         "everything": []}
                for linker in ("link", "lld-link"):
                    for opt in ("REF", "NOREF"):
                        outs = {}
                        for which, library in (("everything", everything), ("original", plain),
                                               ("folded", folded)):
                            # Every link in one directory, of the same file
                            # names (the outputs moved aside after), so that
                            # the links' command lines and module paths (in
                            # the PDBs) are the same.
                            here = os.path.join(d, "link")
                            shutil.rmtree(here, ignore_errors=True)
                            os.makedirs(here)
                            shutil.copyfile(objs["main"], os.path.join(here, "main.obj"))
                            shutil.copyfile(library, os.path.join(here, "lib.lib"))
                            more = []
                            for k, e in enumerate(extra[which]):
                                shutil.copyfile(e, os.path.join(here, f"more{k}.lib"))
                                more.append(f"more{k}.lib")
                            r.must([tools[linker], "/nologo", "/DEBUG", "/INCREMENTAL:NO",
                                    f"/OPT:{opt}", "/OUT:p.exe", "/PDB:p.pdb", "main.obj",
                                    "lib.lib", *more], cwd=here)
                            stem = os.path.join(d, f"{mode}-{case}-{linker}-{opt}-{which}")
                            for ext in (".exe", ".pdb"):
                                os.replace(os.path.join(here, "p" + ext), stem + ext)
                            p = subprocess.run([stem + ".exe"], capture_output=True, text=True,
                                               timeout=TIMEOUT)
                            outs[which] = (p.returncode, p.stdout, stem)
                        if not (outs["everything"][:2] == outs["original"][:2]
                                == outs["folded"][:2]):
                            print(f"{set_name} {mode} {case} {linker} /OPT:{opt}: output DIFFERS")
                            print("  everything:", outs["everything"][:2])
                            print("  original:", outs["original"][:2])
                            print("  folded:  ", outs["folded"][:2])
                            bad += 1
                        o, f = outs["original"][2], outs["folded"][2]
                        code, ctext = r.run([turbolib_exe, "compare-images", o + ".exe",
                                             o + ".pdb", f + ".exe", f + ".pdb"])
                        failed = {line.split(":")[1].strip() for line in ctext.splitlines()
                                  if line.startswith("FAILED:")}
                        key = (set_name, mode, linker, opt)
                        if case == "first":
                            baseline[key] = failed
                            if failed:
                                print(f"{set_name} {mode} first {linker} /OPT:{opt}: "
                                      f"compare-images finds: {sorted(failed)}")
                        elif failed - baseline.get(key, set()):
                            beyond = sorted(failed - baseline.get(key, set()))
                            print(f"{set_name} {mode} {case} {linker} /OPT:{opt}: "
                                  f"compare-images FAILED beyond the first fold's: {beyond}\n"
                                  f"{ctext}")
                            bad += 1
        print(f"{set_name}: done")
    print(f"{'set':10} {'mode':6} {'case':8} {'lib.exe':>9} {'folded':>9} {'saved':>6} removed")
    for s, mode, case, plain, folded, removed in rows:
        print(f"{s:10} {mode:6} {case:8} {plain:9} {folded:9} "
              f"{100 * (plain - folded) / plain:5.1f}% {removed}")
    print(f"{len(rows)} libraries, {bad} failures; log: {r.log}")
    if not a.keep and bad == 0 and not a.work:
        shutil.rmtree(work, ignore_errors=True)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
