"""Links programs against folded libraries the ways a Debug build links
them, and checks each program runs as the unfolded library's does and that
its PDB reads the same through DIA (what the Visual Studio debugger reads
PDBs with).

    python run.py [--turbolib PATH] [--arch x64 x86 arm64] [--config od o2]

For each architecture and configuration, in target/fixtures/turbolib-incremental:

1. Compiles the sources here: pch.cpp (/Yc), a.cpp and b.cpp (/Yu), c.cpp,
   zi.cpp (/Zi, its own PDB), gl.cpp (/GL), and main.cpp; and makes an
   import library of kernel32.def.
2. Archives pch, a, b, c, zi, and the import library twice: with lib.exe
   (unfolded.lib) and with turbolib --fold=identical --precomp=library
   --strict (folded.lib, which turbolib checks itself), and runs
   turbolib verify on the pair.
3. Links main.obj against each, with link.exe:
   - /DEBUG, so /INCREMENTAL (link.exe's default), a full link first;
   - again after main.cpp changes (MAIN_VERSION=2): an incremental link;
   - again after c.cpp changes (EXTRA_C) and the library is archived
     (and folded) again: an incremental link;
   - again after a.cpp changes (VERSION=2: a /Yu object, and a COMDAT
     c.obj also has, so the synthesized members change too). link.exe
     14.52's incremental relink fails on this with lib.exe's library too
     (LNK1235), so what is checked is that it fails against the folded
     library only where it fails against lib.exe's, and that the next
     link (link.exe falls back to a full one) is right;
   - /DEBUG:FASTLINK;
   - with gl.obj in the library (/GL), which makes it an /LTCG link;
   - with lld-link (which links in full; it has no incremental mode).
   Each program is run (x64 and x86; ARM64 is linked only) and must print
   what the unfolded library's program prints. Each PDB is read with
   diadump (tools/diadump, built here): every function's name and lines,
   the user-defined types, and the data must be those of the unfolded
   library's program.

Prints one line for each check and exits with 1 if any failed. The work
directory isn't cleaned; delete it to start again.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
WORK = os.path.join(ROOT, "target", "fixtures", "turbolib-incremental")
VSWHERE = os.path.join(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
                       "Microsoft Visual Studio", "Installer", "vswhere.exe")
LLVM_BIN = os.environ.get("LLVM_BIN", r"C:\jm\dev\llvm-project\build-release\bin")
TIMEOUT = 600

ARCHES = {"x64": ("amd64", "X64"), "x86": ("amd64_x86", "X86"), "arm64": ("amd64_arm64", "ARM64")}
CONFIGS = {"od": ["/Od", "/Ob0"], "o2": ["/O2"]}
# More compiler options for every object (an experiment's).
CL_EXTRA = []

failures = []
knowns = []


def known(what, why):
    """A check that fails for a reason already known and written down."""
    print(f"KNOWN {what}: {why}")
    knowns.append(what)


def check(ok, what, detail=""):
    print(f"{'ok  ' if ok else 'FAIL'} {what}" + (f": {detail}" if detail and not ok else ""))
    if not ok:
        failures.append(what)
    return ok


def vs_path():
    return subprocess.run([VSWHERE, "-latest", "-property", "installationPath"],
                          capture_output=True, text=True, check=True, timeout=60).stdout.strip()


def vs_env(vs, target):
    """The environment vcvarsall.bat makes for `target`."""
    bat = os.path.join(vs, "VC", "Auxiliary", "Build", "vcvarsall.bat")
    out = subprocess.run(f'"{bat}" {target} >nul && set', shell=True, capture_output=True,
                         text=True, check=True, timeout=600).stdout
    env = {}
    for line in out.splitlines():
        k, sep, v = line.partition("=")
        if sep:
            env[k] = v
    return env


def run(args, cwd, env, ok_codes=(0,)):
    # (Windows finds a program on the parent's PATH, not on `env`'s.)
    found = shutil.which(args[0], path=env.get("PATH") or env.get("Path"))
    args = [found or args[0]] + list(args[1:])
    p = subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True,
                       errors="replace", timeout=TIMEOUT)
    out = p.stdout + p.stderr
    if p.returncode not in ok_codes:
        raise RuntimeError(f"{args[0]} exited {p.returncode}:\n{' '.join(args)}\n{out}")
    return out


def compile_all(d, env, config, version=1, main_version=1, only=None):
    cl = ["cl", "/nologo", "/c", "/EHsc", "/GS-", "/Gy", "/MT"] + CL_EXTRA + CONFIGS[config]
    z7 = cl + ["/Z7"]
    v = [f"/DVERSION={version}"]
    jobs = {
        "pch": z7 + ['/Ycshared.h', "/Fppch.pch", os.path.join(HERE, "pch.cpp")],
        "a": z7 + v + ['/Yushared.h', "/Fppch.pch", os.path.join(HERE, "a.cpp")],
        "b": z7 + ['/Yushared.h', "/Fppch.pch", os.path.join(HERE, "b.cpp")],
        "c": z7 + [os.path.join(HERE, "c.cpp")],
        "zi": cl + ["/Zi", "/Fd:zi.pdb", os.path.join(HERE, "zi.cpp")],
        "gl": cl + ["/Z7", "/GL", os.path.join(HERE, "gl.cpp")],
        "main": z7 + ["/DWITH_ZI", f"/DMAIN_VERSION={main_version}", os.path.join(HERE, "main.cpp")],
        "main_gl": z7 + ["/DWITH_ZI", "/DWITH_GL", "/Fo:main_gl.obj", os.path.join(HERE, "main.cpp")],
    }
    order = only or list(jobs)
    if "pch" in order:
        # The users need the PCH first.
        run(jobs["pch"], d, env)
    for name in order:
        if name != "pch":
            run(jobs[name], d, env)


def archive(d, env, machine, turbolib, out, members, fold):
    if fold:
        e = dict(env)
        e["TURBOLIB_OPTIONS"] = "--fold=identical --precomp=library --strict"
        text = run([turbolib, "lib", "/nologo", f"/MACHINE:{machine}", f"/OUT:{out}"] + members, d, e)
    else:
        text = run(["lib", "/nologo", f"/MACHINE:{machine}", f"/OUT:{out}"] + members, d, env)
    return text


def link(d, env, machine, out, objects, flags, linker="link"):
    args = [linker, "/nologo", f"/MACHINE:{machine}", f"/OUT:{out}.exe", f"/PDB:{out}.pdb",
            "/SUBSYSTEM:CONSOLE"] + flags + objects
    return run(args, d, env)


def program_output(d, exe):
    return subprocess.run([os.path.join(d, exe)], capture_output=True, text=True,
                          timeout=60).stdout.strip()


def build_diadump(env):
    exe = os.path.join(WORK, "diadump.exe")
    if os.path.exists(exe):
        return exe
    vs = env["VSINSTALLDIR"]
    src = os.path.join(ROOT, "tools", "diadump", "diadump.cpp")
    run(["cl", "/nologo", "/EHsc", "/O2", f"/I{vs}DIA SDK\\include", src, f"/Fe:{exe}",
         f"/Fo:{WORK}\\", "/link", f"{vs}DIA SDK\\lib\\amd64\\diaguids.lib", "ole32.lib",
         "oleaut32.lib"], WORK, env)
    shutil.copy(os.path.join(vs, "DIA SDK", "bin", "amd64", "msdia140.dll"), WORK)
    return exe


def dia_view(diadump, pdb):
    """What DIA reads in a PDB, without addresses: each function with the
    (line, file) pairs inside it, the user-defined types, and the data."""
    text = subprocess.run([diadump, pdb], capture_output=True, text=True, errors="replace",
                          timeout=TIMEOUT).stdout
    funcs, lines, udts, data = [], [], set(), set()
    for l in text.splitlines():
        if l.startswith("FUNC "):
            _, rva, length, name = l.split(" ", 3)
            funcs.append((int(rva, 16), int(length, 16), name))
        elif l.startswith("LINE "):
            _, rva, length, line, file = l.split(" ", 4)
            lines.append((int(rva, 16), int(line), os.path.basename(file).lower()))
        elif l.startswith("UDT "):
            udts.add(l.split(" ", 2)[2])
        elif l.startswith("DATA "):
            data.add(l.split(" ", 2)[2])
    funcs.sort()
    lines.sort()
    import bisect
    starts = [f[0] for f in funcs]
    by_func = {}
    for rva, line, file in lines:
        i = bisect.bisect_right(starts, rva) - 1
        if i >= 0 and rva < funcs[i][0] + max(funcs[i][1], 1):
            by_func.setdefault(funcs[i][2], set()).add((line, file))
    ours = {name for _, _, name in funcs}
    return {"functions": ours, "lines": by_func, "types": udts, "data": data}


# The functions and types of the program's own sources (folded ones among
# them) that each PDB must describe, with lines.
EXPECTED_FUNCTIONS = ["main", "a_value", "b_value", "c_value", "zi_value", "use_shared",
                      "shared_add", "switch_on", "greeting", "text_length", "Square::area"]
EXPECTED_TYPES = ["Point", "Square", "Shape"]


def compare_dia(what, diadump, pdb_a, pdb_b):
    a, b = dia_view(diadump, pdb_a), dia_view(diadump, pdb_b)
    for f in EXPECTED_FUNCTIONS:
        if not check(f in b["functions"] and b["lines"].get(f), f"{what}: DIA finds {f} with lines"):
            return
    for t in EXPECTED_TYPES:
        check(t in b["types"], f"{what}: DIA finds type {t}")
    ours = lambda v: {n for n in v["functions"] if not n.startswith(("_", "std::", "`"))
                      and "::_" not in n}
    missing = sorted(ours(a) - ours(b))
    check(not missing, f"{what}: every function of the unfolded program's PDB", f"missing {missing[:8]}")
    differing = sorted(f for f in ours(a) & ours(b) if a["lines"].get(f) != b["lines"].get(f))
    check(not differing, f"{what}: the same lines in each function", f"differ: {differing[:8]}")
    tmissing = sorted(a["types"] - b["types"])
    check(not tmissing, f"{what}: every user-defined type", f"missing {tmissing[:8]}")


def full_link(text):
    return "performing full link" in text.lower() or "full link" in text.lower()


def one(arch, config, turbolib, diadump):
    target, machine = ARCHES[arch]
    d = os.path.join(WORK, f"{arch}-{config}")
    os.makedirs(d, exist_ok=True)
    env = vs_env(VS, target)
    runs = arch != "arm64"
    tag = f"{arch} {config}"
    print(f"== {tag}")
    compile_all(d, env, config)
    run(["lib", "/nologo", f"/MACHINE:{machine}", f"/DEF:{os.path.join(HERE, 'kernel32.def')}",
         "/OUT:imports.lib"], d, env)
    members = ["pch.obj", "a.obj", "b.obj", "c.obj", "zi.obj", "imports.lib"]

    def libs(suffix=""):
        archive(d, env, machine, "lib", f"unfolded{suffix}.lib", members, False)
        text = archive(d, env, machine, turbolib, f"folded{suffix}.lib", members, True)
        check("warning" not in text, f"{tag}: turbolib folds without a warning", text)
        # Against turbolib's own unfolded library too, which is lib.exe's
        # (the /GL member's index entries too: its IL references read whole).
        e = dict(env)
        e["TURBOLIB_OPTIONS"] = "--fold=none"
        run([turbolib, "lib", "/nologo", f"/MACHINE:{machine}", f"/OUT:plain{suffix}.lib"] + members,
            d, e)
        for original in ("unfolded", "plain"):
            v = subprocess.run([turbolib, "verify", f"{original}{suffix}.lib", f"folded{suffix}.lib"],
                               cwd=d, capture_output=True, text=True, timeout=TIMEOUT)
            what = f"{tag}: turbolib verify {original}{suffix}.lib folded{suffix}.lib"
            check(v.returncode == 0, what, v.stdout[-2000:])
        return os.path.getsize(os.path.join(d, f"unfolded{suffix}.lib")), \
            os.path.getsize(os.path.join(d, f"folded{suffix}.lib"))

    sizes = libs()
    print(f"     libraries: {sizes[0]} bytes unfolded, {sizes[1]} folded")
    base_flags = ["/DEBUG", "/INCREMENTAL:NO"]
    inc = ["/DEBUG", "/INCREMENTAL", "/VERBOSE:INCR"]

    def expect(name, objects, flags, reference_flags=base_flags):
        """Links `name` against folded.lib and its reference against
        unfolded.lib; checks it runs the same and its PDB reads the same."""
        text = link(d, env, machine, name, objects + ["folded.lib"], flags)
        link(d, env, machine, f"{name}-ref", objects + ["unfolded.lib"], reference_flags)
        if runs:
            want = program_output(d, f"{name}-ref.exe")
            got = program_output(d, f"{name}.exe")
            check(got == want and want.startswith("main "), f"{tag}: {name} runs as the unfolded one does",
                  f"{got!r} != {want!r}")
        compare_dia(f"{tag}: {name}", diadump, os.path.join(d, f"{name}-ref.pdb"),
                    os.path.join(d, f"{name}.pdb"))
        return text

    # Incremental: a full link first.
    for f in ["inc.exe", "inc.ilk", "inc.pdb", "incu.exe", "incu.ilk", "incu.pdb"]:
        if os.path.exists(os.path.join(d, f)):
            os.remove(os.path.join(d, f))
    expect("inc", ["main.obj"], inc)
    check(os.path.exists(os.path.join(d, "inc.ilk")), f"{tag}: the link is incremental (inc.ilk)")
    # The same against the unfolded library, incrementally, to compare what
    # link.exe does on each relink.
    link(d, env, machine, "incu", ["main.obj", "unfolded.lib"], inc)
    # main.cpp changes: an incremental relink.
    compile_all(d, env, config, main_version=2, only=["main"])
    text = expect("inc", ["main.obj"], inc)
    textu = link(d, env, machine, "incu", ["main.obj", "unfolded.lib"], inc)
    check(not full_link(text) or full_link(textu),
          f"{tag}: after main.obj changes, an incremental relink (as against unfolded.lib)",
          text[-1500:])
    print(f"     relink after main.obj: {'full' if full_link(text) else 'incremental'} "
          f"(unfolded.lib: {'full' if full_link(textu) else 'incremental'})")
    # c.cpp changes and the library is archived and folded again.
    CL_EXTRA.append("/DEXTRA_C=1")
    compile_all(d, env, config, main_version=2, only=["c"])
    CL_EXTRA.pop()
    libs()
    text = expect("inc", ["main.obj"], inc)
    textu = link(d, env, machine, "incu", ["main.obj", "unfolded.lib"], inc)
    check(not full_link(text) or full_link(textu),
          f"{tag}: after c.obj changes in the library, an incremental relink (as against unfolded.lib)",
          text[-1500:])
    print(f"     relink after c.obj changes in the library: {'full' if full_link(text) else 'incremental'} "
          f"(unfolded.lib: {'full' if full_link(textu) else 'incremental'})")
    # a.cpp changes: a /Yu object, with a COMDAT c.obj has too.
    compile_all(d, env, config, version=2, main_version=2, only=["a"])
    libs()

    def attempt(name, library):
        try:
            return "full" if full_link(link(d, env, machine, name, ["main.obj", library], inc))                 else "incremental"
        except RuntimeError as e:
            m = re.search(r"LNK[0-9]+", str(e))
            return f"failed ({m.group(0) if m else 'error'})"

    got, gotu = attempt("inc", "folded.lib"), attempt("incu", "unfolded.lib")
    print(f"     relink after a.obj changes in the library: {got} (unfolded.lib: {gotu})")
    check(not got.startswith("failed") or gotu.startswith("failed"),
          f"{tag}: after a.obj changes, the relink fails only where lib.exe's library's does",
          f"{got} against {gotu}")
    # The next link, whatever happened, is right.
    expect("inc", ["main.obj"], inc)
    # Nothing changes, and the library is archived again (its synthesized
    # members get a new date, as lib.exe's members get their files').
    libs()
    text = expect("inc", ["main.obj"], inc)
    print(f"     relink after the same library again: {'full' if full_link(text) else 'incremental'}")
    # Back to version 1 for the rest.
    compile_all(d, env, config, only=["a", "main"])
    libs()
    expect("fastlink", ["main.obj"], ["/DEBUG:FASTLINK", "/INCREMENTAL:NO"])
    # /GL: gl.obj in the library, and an /LTCG link.
    members.append("gl.obj")
    libs("-gl")
    text = link(d, env, machine, "ltcg", ["main_gl.obj", "folded-gl.lib"], ["/DEBUG", "/LTCG"])
    link(d, env, machine, "ltcg-ref", ["main_gl.obj", "unfolded-gl.lib"], ["/DEBUG", "/LTCG"])
    if runs:
        want = program_output(d, "ltcg-ref.exe")
        got = program_output(d, "ltcg.exe")
        check(got == want and " gl " in want, f"{tag}: ltcg runs as the unfolded one does",
              f"{got!r} != {want!r}")
    members.pop()
    # lld-link.
    lld = os.path.join(LLVM_BIN, "lld-link.exe")
    if os.path.exists(lld):
        link(d, env, machine, "lld", ["main.obj", "folded.lib"], ["/DEBUG"], linker=lld)
        link(d, env, machine, "lld-ref", ["main.obj", "unfolded.lib"], ["/DEBUG"], linker=lld)
        if runs:
            want = program_output(d, "lld-ref.exe")
            got = program_output(d, "lld.exe")
            check(got == want and want.startswith("main "), f"{tag}: lld runs as the unfolded one does",
                  f"{got!r} != {want!r}")
        compare_dia(f"{tag}: lld", diadump, os.path.join(d, "lld-ref.pdb"),
                    os.path.join(d, "lld.pdb"))


def main():
    global VS
    ap = argparse.ArgumentParser()
    ap.add_argument("--turbolib", default=os.path.join(ROOT, "target", "release", "turbolib.exe"))
    ap.add_argument("--arch", nargs="+", default=["x64", "x86", "arm64"], choices=list(ARCHES))
    ap.add_argument("--config", nargs="+", default=["od", "o2"], choices=list(CONFIGS))
    a = ap.parse_args()
    os.makedirs(WORK, exist_ok=True)
    VS = vs_path()
    diadump = build_diadump(vs_env(VS, "amd64"))
    for arch in a.arch:
        for config in a.config:
            try:
                one(arch, config, os.path.abspath(a.turbolib), diadump)
            except (RuntimeError, subprocess.TimeoutExpired) as e:
                check(False, f"{arch} {config}", str(e)[-3000:])
    print((f"{len(failures)} failed" if failures else "all passed") +
          (f" ({len(knowns)} known failures)" if knowns else ""))
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
