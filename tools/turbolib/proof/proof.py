"""Builds LLVM, clang, and lld from scratch with lib.exe or turbolib as the
archiver, runs their test suites, and measures the builds: the proof that
turbolib can stand in for lib.exe on a large /Z7 project.

    python proof.py configure <mode> <build dir> [--linker link|lld]
    python proof.py clone <built dir> <build dir>
    python proof.py build <mode> <build dir> [-j N] [--timeout S] [target...]
    python proof.py test <mode> <build dir> <results dir> [suite...] [--timeout S]
                    [--filter-out-file FILE] [--lit-opt=OPTION...]
    python proof.py stats <build dir> [--json FILE]
    python proof.py measure <mode> <build dir> <target>...
    python proof.py compare <base results> <other results>

<mode> is one of
  base        lib.exe (the build's own, found by CMake);
  fold        turbolib as lib.exe, TURBOLIB_OPTIONS=--fold=identical;
  fold-types  turbolib as lib.exe, /TURBOLIB:fold=identical and
              /TURBOLIB:precomp=library in CMAKE_STATIC_LINKER_FLAGS (the
              other spelling, so both are exercised).

turbolib is the binary at --turbolib (default: this repository's
target\\release\\turbolib.exe), copied once to <build dir>\\..\\turbolib-bin\\
<mode>\\lib.exe: turbolib picks its `lib` personality from argv[0], and the
copy keeps a rebuild of the repository from changing a build half way.

`clone` copies a built directory (the baseline) for a build with another
archiver, so that only the libraries and the programs are made again: its
objects, generated files, and Ninja's logs, without its libraries,
programs, PDBs, and test outputs. The copy must be as deep as the original
(Ninja logs paths relative to the build directory). The `configure` that
follows rewrites Ninja's log so that a compile whose command differs only
in the build directory's own include, PCH, and forced-include paths counts
as done; every archive, link, and custom command (tablegen, whose
--write-if-changed outputs then stay as they were) runs again.

`build` runs Ninja in the Visual Studio environment (vcvars64.bat) and
appends the wall time to <build dir>\\proof-times.json. `test` runs the
check targets (check-llvm, check-clang, check-lld by default) with lit's
xunit output in <results dir>; --filter-out-file names the tests to leave
out (clang-slow.re: Debug clang's slowest). Both run Ninja in a job object
and stop every process in it after --timeout seconds. `stats` reads .ninja_log: every step's time,
the archiver steps' (static libraries) total, the big programs' links, and
the libraries' sizes. `measure` deletes each target's image, PDB, and
incremental-link file and relinks it with Ninja (the build's own command,
/INCREMENTAL as configured) inside a job object, reporting the time and the
peak commit of the largest process. `compare` lists the tests that pass in
the first results and not in the second.
"""
import argparse
import ctypes
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from ctypes import wintypes

VCVARS = r"C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat"
LLVM_SRC = os.environ.get("LLVM_SRC", r"C:\jm\dev\llvm-project")
REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))
TURBOLIB = os.path.join(REPO, "target", "release", "turbolib.exe")
LIT_TOOLS = r"C:\Program Files\Git\usr\bin"
LLD = os.path.join(os.environ.get("LLVM_BIN", os.path.join(LLVM_SRC, "build-release", "bin")),
                   "lld-link.exe")
MODES = ("base", "fold", "fold-types")
BIG = ("bin/clang.exe", "bin/lld.exe", "bin/llvm-objdump.exe", "bin/opt.exe", "bin/llc.exe",
       "bin/clang-repl.exe", "bin/clang-scan-deps.exe", "bin/llvm-readobj.exe")


def vs_env():
    """The environment vcvars64.bat sets up."""
    out = subprocess.run(f'"{VCVARS}" >nul && set', capture_output=True,
                         text=True, shell=True, check=True).stdout
    env = {}
    for line in out.splitlines():
        k, sep, v = line.partition("=")
        if sep:
            env[k] = v
    return env


def tool(name, env):
    """A program's path, found on the environment's PATH (CreateProcess
    searches the parent's)."""
    found = shutil.which(name, path=env.get("PATH") or env.get("Path"))
    if not found:
        raise SystemExit(f"{name}: not found on PATH")
    return found


def mode_env(mode, env):
    env = dict(env)
    env.pop("TURBOLIB_OPTIONS", None)
    if mode == "fold":
        env["TURBOLIB_OPTIONS"] = "--fold=identical"
    return env


def turbolib_copy(build_dir, mode, source):
    d = os.path.join(os.path.dirname(os.path.abspath(build_dir)), "turbolib-bin", mode)
    os.makedirs(d, exist_ok=True)
    dst = os.path.join(d, "lib.exe")
    if not os.path.exists(dst):
        shutil.copyfile(source, dst)
        pdb = os.path.splitext(source)[0] + ".pdb"
        if os.path.exists(pdb):
            shutil.copyfile(pdb, os.path.join(d, "lib.pdb"))
    return dst


def configure(a):
    env = mode_env(a.mode, vs_env())
    args = [tool("cmake", env), "-G", "Ninja", "-S", os.path.join(LLVM_SRC, "llvm"), "-B", a.build_dir,
            "-DCMAKE_BUILD_TYPE=Debug",
            "-DCMAKE_POLICY_DEFAULT_CMP0141=NEW",
            "-DCMAKE_MSVC_DEBUG_INFORMATION_FORMAT=Embedded",
            "-DLLVM_ENABLE_PROJECTS=clang;lld",
            "-DLLVM_TARGETS_TO_BUILD=X86;AArch64",
            "-DLLVM_OPTIMIZED_TABLEGEN=ON",
            "-DLLVM_INCLUDE_TESTS=ON", "-DLLVM_BUILD_TESTS=ON",
            "-DCLANG_INCLUDE_TESTS=ON",
            "-DLLVM_INCLUDE_BENCHMARKS=OFF", "-DLLVM_INCLUDE_EXAMPLES=OFF",
            f"-DLLVM_LIT_TOOLS_DIR={LIT_TOOLS}",
            f"-DLLVM_PARALLEL_LINK_JOBS={a.link_jobs}"]
    if a.mode != "base":
        archiver = turbolib_copy(a.build_dir, a.mode, a.turbolib)
        args.append("-DCMAKE_AR=" + archiver)
        # A configured directory (a clone) keeps the archiver it found
        # first: CMakeFiles/<version>/CMake<lang>Compiler.cmake sets
        # CMAKE_AR over the cache's. Name the new one there too.
        for d in glob.glob(os.path.join(a.build_dir, "CMakeFiles", "*", "CMake*Compiler.cmake")):
            with open(d, encoding="utf-8") as f:
                text = f.read()
            new = re.sub(r'^set\(CMAKE_AR "[^"]*"\)$',
                         lambda m: f'set(CMAKE_AR "{archiver.replace(chr(92), "/")}")', text,
                         flags=re.M)
            if new != text:
                with open(d, "w", encoding="utf-8") as f:
                    f.write(new)
    if a.mode == "fold-types":
        args.append("-DCMAKE_STATIC_LINKER_FLAGS=/machine:x64 /TURBOLIB:fold=identical "
                    "/TURBOLIB:precomp=library")
    if a.linker == "lld":
        args.append("-DCMAKE_LINKER=" + LLD)
    print(" ".join(args), flush=True)
    rc = subprocess.run(args, env=env).returncode
    mark = os.path.join(a.build_dir, CLONE_MARK)
    if rc == 0 and os.path.exists(mark):
        with open(mark) as f:
            source = json.load(f)["source"]
        rehash(source, os.path.abspath(a.build_dir), env)
        os.rename(mark, mark + ".done")
    sys.exit(rc)


CLONE_MARK = "proof-clone.json"
COMPILE_RULES = ("CXX_COMPILER__", "C_COMPILER__", "RC_COMPILER__", "ASM_MASM_COMPILER__")
# The flags in which a compile command may name its build directory and
# still compile the same: the build's own headers, PCH, and forced include.
PATH_FLAGS = ("-I", "/I", "/Yu", "/Yc", "/Fp", "/FI", "-external:I", "/external:I")


def clone(a):
    """Copies a configured build directory for a build with another
    archiver: its objects, generated files, and logs, without its libraries,
    programs, PDBs, and test outputs, which the new build makes again. The
    tablegen build (NATIVE) is copied whole and reused. `configure` then
    regenerates the copy and keeps the compiles whose commands differ only
    in the build directory's own include, PCH and forced-include paths (see
    rehash), so the build archives every library and links every program
    again and compiles nothing else."""
    src, dst = os.path.abspath(a.source), os.path.abspath(a.build_dir)
    if os.path.relpath(LLVM_SRC, src) != os.path.relpath(LLVM_SRC, dst):
        # Ninja's logs hold paths relative to the build directory.
        raise SystemExit(f"{dst} must be as deep as {src}: the paths Ninja logged are relative")
    if os.path.exists(dst):
        raise SystemExit(f"{dst} exists")
    native = os.path.join(src, "NATIVE")
    rc = subprocess.run(["robocopy", src, dst, "/E", "/COPY:DAT", "/DCOPY:T", "/MT:8", "/NFL", "/NDL",
                         "/NP", "/R:1", "/W:1", "/XD", native, "Output", "/XF", "*.lib", "*.exe",
                         "*.ilk", "*.pdb", "*.dll", "*.exp", "proof-times.json"]).returncode
    if rc >= 8:
        raise SystemExit(f"robocopy: exit {rc}")
    rc = subprocess.run(["robocopy", native, os.path.join(dst, "NATIVE"), "/E", "/COPY:DAT", "/DCOPY:T",
                         "/MT:8", "/NFL", "/NDL", "/NP", "/R:1", "/W:1"]).returncode
    if rc >= 8:
        raise SystemExit(f"robocopy: exit {rc}")
    for cache in ("CMakeCache.txt", os.path.join("NATIVE", "CMakeCache.txt")):
        path = os.path.join(dst, cache)
        with open(path, encoding="utf-8", errors="surrogateescape") as f:
            text = f.read()
        with open(path, "w", encoding="utf-8", errors="surrogateescape") as f:
            f.write(swap_root(text, src, dst))
    with open(os.path.join(dst, CLONE_MARK), "w") as f:
        json.dump({"source": src}, f)
    # The tablegen build: regenerated here and reused whole (it is a build
    # tool, archived by lib.exe in every mode).
    env = vs_env()
    native_dst = os.path.join(dst, "NATIVE")
    r = subprocess.run([tool("cmake", env), native_dst], env=env)
    if r.returncode:
        raise SystemExit(f"cmake {native_dst}: exit {r.returncode}")
    rehash(native, native_dst, env, every_rule=True)


def root_pattern(root):
    drive, rest = os.path.splitdrive(root)
    parts = [p for p in re.split(r"[\\/]", rest) if p]
    return re.compile(re.escape(drive) + "".join(r"([\\/])" + re.escape(p) if i == 0 else
                                                 r"[\\/]" + re.escape(p)
                                                 for i, p in enumerate(parts)) + r"(?![\w.-])",
                      re.I)


def swap_root(text, old, new):
    """`text` with the directory `old` written as `new`, in either slash."""
    drive, rest = os.path.splitdrive(new)
    parts = [p for p in re.split(r"[\\/]", rest) if p]
    return root_pattern(old).sub(lambda m: m.group(0)[:len(drive)] + "".join(
        m.group(1) + p for p in parts), text)


MASK64 = (1 << 64) - 1
RAPID_SECRET = (0x2D358DCCAA6C78A5, 0x8BB84B93962EACC9, 0x4B33A62ED433D4A3)


def rapid_mum(a, b):
    p = a * b
    return p & MASK64, p >> 64


def rapid_mix(a, b):
    lo, hi = rapid_mum(a, b)
    return lo ^ hi


def command_hash(data):
    """Ninja's hash of a command in a v7 log (rapidhash, as
    BuildLog::LogEntry::HashCommand computes it since Ninja 1.13)."""
    s = RAPID_SECRET
    n = len(data)

    def r64(i):
        return int.from_bytes(data[i:i + 8], "little")

    def r32(i):
        return int.from_bytes(data[i:i + 4], "little")

    seed = 0xBDD89AA982704029
    seed ^= rapid_mix(seed ^ s[0], s[1]) ^ n
    if n <= 16:
        if n >= 4:
            last = n - 4
            a = (r32(0) << 32) | r32(last)
            delta = (n & 24) >> (n >> 3)
            b = (r32(delta) << 32) | r32(last - delta)
        elif n > 0:
            a = (data[0] << 56) | (data[n >> 1] << 32) | data[n - 1]
            b = 0
        else:
            a = b = 0
    else:
        p, i = 0, n
        if i > 48:
            see1 = see2 = seed
            while i >= 48:
                seed = rapid_mix(r64(p) ^ s[0], r64(p + 8) ^ seed)
                see1 = rapid_mix(r64(p + 16) ^ s[1], r64(p + 24) ^ see1)
                see2 = rapid_mix(r64(p + 32) ^ s[2], r64(p + 40) ^ see2)
                p += 48
                i -= 48
            seed ^= see1 ^ see2
        if i > 16:
            seed = rapid_mix(r64(p) ^ s[2], r64(p + 8) ^ seed ^ s[1])
            if i > 32:
                seed = rapid_mix(r64(p + 16) ^ s[2], r64(p + 24) ^ seed)
        a, b = r64(p + i - 16), r64(p + i - 8)
    a, b = rapid_mum(a ^ s[1], b ^ seed)
    return rapid_mix(a ^ s[0] ^ n, b ^ s[1])


def compdb(build_dir, env):
    """[(rule, output, command)] of every edge with a command."""
    out = subprocess.run([tool("ninja", env), "-C", build_dir, "-t", "compdb"], env=env,
                         capture_output=True, check=True).stdout
    entries = json.loads(out.decode("utf-8", errors="surrogateescape"))
    rules = {}
    with open(os.path.join(build_dir, "build.ninja"), encoding="utf-8", errors="replace") as f:
        for line in f:
            m = re.match(r"build (.+?)(?<!\$): (\S+)", line)
            if m:
                first = re.split(r"(?<!\$) ", m.group(1))[0]
                rules[first.replace("$:", ":").replace("$ ", " ").replace("/", "\\")] = m.group(2)
    return [(rules.get(e["output"].replace("/", "\\"), "?"), e["output"], e["command"])
            for e in entries]


def rehash(src, dst, env, every_rule=False):
    """Rewrites dst's .ninja_log so that the edges whose commands differ
    from src's only by the build directory (in PATH_FLAGS, for compiles)
    count as done: their outputs, copied from src, are what the new command
    would make. Every other edge (archives, links, custom commands, and
    compiles with the directory elsewhere, as in a -D) runs again."""
    old_hash = {}
    log = os.path.join(dst, ".ninja_log")
    with open(log, encoding="utf-8", errors="surrogateescape") as f:
        lines = f.readlines()
    for line in lines[1:]:
        p = line.rstrip("\n").split("\t")
        if len(p) >= 5:
            old_hash[p[3].replace("/", "\\")] = p[4]
    new_root = root_pattern(dst)
    remap, kept, rerun = {}, 0, 0
    same_cache = {}

    def same_file(token):
        """Whether a token is a file of dst with the bytes of src's (a
        generated source, such as cmake_pch.cxx or a tool's driver)."""
        if token not in same_cache:
            p, q = token.strip('"'), swap_root(token.strip('"'), dst, src)
            same = False
            if os.path.isfile(p) and os.path.isfile(q):
                with open(p, "rb") as f, open(q, "rb") as g:
                    same = f.read() == g.read()
            same_cache[token] = same
        return same_cache[token]

    for rule, output, command in compdb(dst, env):
        logged = old_hash.get(output.replace("/", "\\"))
        if logged is None:
            continue
        if not every_rule and not rule.startswith(COMPILE_RULES):
            rerun += 1
            continue
        if f"{command_hash(command.encode('utf-8', errors='surrogateescape')):x}" == logged:
            kept += 1  # kept by an earlier rehash
            continue
        if not every_rule:
            words = command.split()
            elsewhere = [t for i, t in enumerate(words) if new_root.search(t)
                         and not t.startswith(PATH_FLAGS) and not same_file(t)
                         and not (i and words[i - 1] in PATH_FLAGS)]
            if elsewhere:
                print(f"proof: compiled again (the directory is in {elsewhere[0]}): {output}")
                rerun += 1
                continue
        back = swap_root(command, dst, src).encode("utf-8", errors="surrogateescape")
        if f"{command_hash(back):x}" != logged:
            rerun += 1
            continue
        remap[logged] = f"{command_hash(command.encode('utf-8', errors='surrogateescape')):x}"
        kept += 1
    with open(log + ".tmp", "w", encoding="utf-8", errors="surrogateescape", newline="\n") as f:
        f.write(lines[0])
        for line in lines[1:]:
            p = line.rstrip("\n").split("\t")
            if len(p) >= 5 and p[4] in remap:
                p[4] = remap[p[4]]
                line = "\t".join(p) + "\n"
            f.write(line)
    os.replace(log + ".tmp", log)
    print(f"proof: {dst}: {kept} steps kept from {src}, {rerun} to run again")


def record_time(build_dir, entry):
    path = os.path.join(build_dir, "proof-times.json")
    times = []
    if os.path.exists(path):
        with open(path) as f:
            times = json.load(f)
    times.append(entry)
    with open(path, "w") as f:
        json.dump(times, f, indent=1)


def build(a):
    env = mode_env(a.mode, vs_env())
    args = [tool("ninja", env), "-C", a.build_dir, f"-j{a.jobs}"] + (["-k", "0"] if a.keep_going else [])
    args += a.targets
    start = time.time()
    rc = run_in_job(args, env, a.timeout)[1]
    seconds = time.time() - start
    record_time(a.build_dir, {"targets": a.targets or ["all"], "jobs": a.jobs,
                              "seconds": round(seconds, 1), "exit": rc,
                              "started": time.strftime("%Y-%m-%d %H:%M:%S",
                                                       time.localtime(start))})
    print(f"proof: ninja {' '.join(a.targets) or 'all'}: {seconds:.0f} s, exit {rc}")
    sys.exit(rc)


def test(a):
    env = mode_env(a.mode, vs_env())
    os.makedirs(a.results, exist_ok=True)
    rc = 0
    for suite in a.suites:
        xml = os.path.join(os.path.abspath(a.results), suite + ".xml")
        e = dict(env)
        # lit splits LIT_OPTS as a POSIX shell would: backslashes would go.
        e["LIT_OPTS"] = (f"--xunit-xml-output={xml.replace(chr(92), '/')} --no-progress-bar -q "
                         + " ".join(a.lit_opts))
        if a.filter_out_file:
            # lit reads LIT_FILTER_OUT unsplit, so the regex keeps its backslashes.
            e["LIT_FILTER_OUT"] = filter_out(a.filter_out_file)
        start = time.time()
        log = os.path.join(a.results, suite + ".log")
        with open(log, "w", encoding="utf-8", errors="replace") as f:
            f.write(f"proof: LIT_OPTS={e['LIT_OPTS']}\n")
            f.flush()
            args = [tool("ninja", env), "-C", a.build_dir, f"-j{a.jobs}", suite]
            if a.lit_only:
                # The check target's own command (lit), without building
                # what it depends on first.
                last = subprocess.run(args[:3] + ["-t", "commands", "-s", suite], env=e,
                                      capture_output=True, text=True, check=True).stdout
                args = last.strip().splitlines()[-1]  # cmd.exe /C "...", as Ninja runs it
                f.write(f"proof: {args}\n")
                f.flush()
            r = run_in_job(args, e, a.timeout, stdout=f)[1]
        seconds = time.time() - start
        record_time(a.build_dir, {"targets": [suite], "lit_opts": a.lit_opts,
                                  "filter_out_file": a.filter_out_file,
                                  "seconds": round(seconds, 1), "exit": r,
                                  "started": time.strftime("%Y-%m-%d %H:%M:%S",
                                                           time.localtime(start))})
        print(f"proof: {suite}: {seconds:.0f} s, exit {r}; {summary(xml)}", flush=True)
        rc = rc or r
    sys.exit(rc)


def filter_out(path):
    """lit's --filter-out regex from a file of regexes, one a line, each
    matched against a test's path within its suite."""
    with open(path, encoding="utf-8") as f:
        lines = [l.strip() for l in f if l.strip() and not l.startswith("#")]
    return r"^\S+ :: (?:" + "|".join(lines) + ")"


def outcomes(xml):
    """{test name: outcome} from lit's xunit output."""
    if not os.path.exists(xml):
        return {}
    out = {}
    for case in ET.parse(xml).getroot().iter("testcase"):
        name = case.get("classname", "") + "::" + case.get("name", "")
        if case.find("failure") is not None or case.find("error") is not None:
            out[name] = "fail"
        elif case.find("skipped") is not None:
            out[name] = "skip"
        else:
            out[name] = "pass"
    return out


def summary(xml):
    o = outcomes(xml)
    counts = {}
    for v in o.values():
        counts[v] = counts.get(v, 0) + 1
    return ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) or "no results"


def compare(a):
    bad = 0
    for name in sorted(os.listdir(a.base)):
        if not name.endswith(".xml"):
            continue
        base = outcomes(os.path.join(a.base, name))
        other = outcomes(os.path.join(a.other, name))
        missing = [t for t in base if t not in other]
        worse = [t for t, v in base.items() if v == "pass" and other.get(t, "pass") != "pass"]
        better = [t for t, v in base.items() if v == "fail" and other.get(t) == "pass"]
        print(f"{name}: base {summary(os.path.join(a.base, name))}; "
              f"other {summary(os.path.join(a.other, name))}; "
              f"{len(worse)} pass only in base, {len(better)} pass only in other, "
              f"{len(missing)} missing")
        for t in worse:
            print(f"  WORSE {t}: {other[t]}")
        for t in better:
            print(f"  better {t}")
        for t in missing[:50]:
            print(f"  missing {t}")
        bad += len(worse) + len(missing)
    sys.exit(1 if bad else 0)


def ninja_log(build_dir):
    """{output: (start ms, end ms)} of the last run of each step."""
    steps = {}
    with open(os.path.join(build_dir, ".ninja_log"), encoding="utf-8", errors="replace") as f:
        for line in f:
            if line.startswith("#"):
                continue
            parts = line.rstrip("\n").split("\t")
            if len(parts) >= 4:
                steps[parts[3].replace("/", "\\")] = (int(parts[0]), int(parts[1]))
    return steps


def static_libraries(build_dir):
    out = []
    with open(os.path.join(build_dir, "build.ninja"), encoding="utf-8", errors="replace") as f:
        for line in f:
            m = re.match(r"build (\S+?): \S*STATIC_LIBRARY_LINKER", line)
            if m:
                out.append(m.group(1).replace("$:", ":"))
    return out


def stats(a):
    steps = ninja_log(a.build_dir)
    libs = static_libraries(a.build_dir)
    lib_ms = [steps[l][1] - steps[l][0] for l in libs if l in steps]
    total_ms = sum(e - s for s, e in steps.values())
    sizes = {l: os.path.getsize(os.path.join(a.build_dir, l)) for l in libs
             if os.path.exists(os.path.join(a.build_dir, l))}
    links = {}
    for t in BIG:
        k = t.replace("/", "\\")
        if k in steps:
            links[t] = (steps[k][1] - steps[k][0]) / 1000
    times_path = os.path.join(a.build_dir, "proof-times.json")
    times = json.load(open(times_path)) if os.path.exists(times_path) else []
    result = {
        "steps": len(steps),
        "step_seconds_total": round(total_ms / 1000, 1),
        "libraries": len(sizes),
        "library_steps_logged": len(lib_ms),
        "library_seconds_total": round(sum(lib_ms) / 1000, 1),
        "library_seconds_max": round(max(lib_ms, default=0) / 1000, 1),
        "library_bytes_total": sum(sizes.values()),
        "largest_libraries": sorted(sizes.items(), key=lambda kv: -kv[1])[:20],
        "link_seconds": links,
        "runs": times,
    }
    text = json.dumps(result, indent=1)
    print(text)
    if a.json:
        with open(a.json, "w") as f:
            f.write(text)
    if a.sizes:
        with open(a.sizes, "w") as f:
            json.dump(sizes, f, indent=0, sort_keys=True)


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(n, ctypes.c_ulonglong) for n in
                ("ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
                 "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]


class BASIC_LIMIT(ctypes.Structure):
    _fields_ = [("PerProcessUserTimeLimit", ctypes.c_longlong),
                ("PerJobUserTimeLimit", ctypes.c_longlong),
                ("LimitFlags", wintypes.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", wintypes.DWORD),
                ("Affinity", ctypes.c_size_t), ("PriorityClass", wintypes.DWORD),
                ("SchedulingClass", wintypes.DWORD)]


class EXTENDED_LIMIT(ctypes.Structure):
    _fields_ = [("BasicLimitInformation", BASIC_LIMIT), ("IoInfo", IO_COUNTERS),
                ("ProcessMemoryLimit", ctypes.c_size_t), ("JobMemoryLimit", ctypes.c_size_t),
                ("PeakProcessMemoryUsed", ctypes.c_size_t),
                ("PeakJobMemoryUsed", ctypes.c_size_t)]


def run_in_job(args, env, timeout=None, stdout=None):
    """Runs a command in a new job that dies with it: (seconds, exit, peak
    process commit, peak job commit). Past `timeout` seconds every process
    in the job is killed (exit 124): lit's workers and the compilers Ninja
    started go too, which killing Ninja alone would leave running."""
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreateJobObjectW.restype = wintypes.HANDLE
    job = k32.CreateJobObjectW(None, None)
    limit = EXTENDED_LIMIT()
    limit.BasicLimitInformation.LimitFlags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    if not k32.SetInformationJobObject(wintypes.HANDLE(job), 9, ctypes.byref(limit),
                                       ctypes.sizeof(limit)):
        raise SystemExit(f"SetInformationJobObject: {ctypes.get_last_error()}")
    start = time.perf_counter()
    # Suspended, so that every child process is born in the job.
    p = subprocess.Popen(args, env=env, stdout=stdout,
                         stderr=subprocess.STDOUT if stdout else None,
                         creationflags=0x4)  # CREATE_SUSPENDED
    if not k32.AssignProcessToJobObject(wintypes.HANDLE(job), wintypes.HANDLE(int(p._handle))):
        raise SystemExit(f"AssignProcessToJobObject: {ctypes.get_last_error()}")
    resume_main_thread(p.pid)
    try:
        rc = p.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        shown = args if isinstance(args, str) else " ".join(args)
        print(f"proof: {timeout} s passed: stopping {shown}", flush=True)
        k32.TerminateJobObject(wintypes.HANDLE(job), 124)
        rc = p.wait()
        rc = 124
    seconds = time.perf_counter() - start
    info = EXTENDED_LIMIT()
    k32.QueryInformationJobObject(wintypes.HANDLE(job), 9, ctypes.byref(info),
                                  ctypes.sizeof(info), None)
    k32.CloseHandle(wintypes.HANDLE(job))
    return seconds, rc, info.PeakProcessMemoryUsed, info.PeakJobMemoryUsed


def resume_main_thread(pid):
    """Resumes the threads of a suspended process (NtResumeProcess)."""
    ntdll = ctypes.WinDLL("ntdll")
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.OpenProcess.restype = wintypes.HANDLE
    h = k32.OpenProcess(0x0800, False, pid)  # PROCESS_SUSPEND_RESUME
    ntdll.NtResumeProcess(wintypes.HANDLE(h))
    k32.CloseHandle(wintypes.HANDLE(h))


def measure(a):
    env = mode_env(a.mode, vs_env())
    results = []
    for t in a.targets:
        base = os.path.join(a.build_dir, os.path.splitext(t)[0])
        for ext in (".exe", ".pdb", ".ilk"):
            if os.path.exists(base + ext):
                os.remove(base + ext)
        seconds, rc, peak, job_peak = run_in_job([tool("ninja", env), "-C", a.build_dir, "-j1", t], env)
        r = {"target": t, "seconds": round(seconds, 2), "exit": rc,
             "peak_process_commit": peak, "peak_job_commit": job_peak,
             "exe_bytes": os.path.getsize(base + ".exe") if os.path.exists(base + ".exe") else 0,
             "pdb_bytes": os.path.getsize(base + ".pdb") if os.path.exists(base + ".pdb") else 0}
        print(json.dumps(r), flush=True)
        results.append(r)
    if a.json:
        with open(a.json, "w") as f:
            json.dump(results, f, indent=1)


def relink(a):
    """Links a program of the build again, as its build links it but into
    another directory and with more options (as /INCREMENTAL:NO): the link
    command with its response file expanded (ninja -t compdb -x), without
    CMake's manifest step. Reads the build directory only."""
    env = vs_env()
    os.makedirs(a.out_dir, exist_ok=True)
    out = subprocess.run([tool("ninja", env), "-C", a.build_dir, "-t", "compdb", "-x"], env=env,
                         capture_output=True, check=True).stdout
    entries = {e["output"].replace("/", "\\"): e
               for e in json.loads(out.decode("utf-8", errors="surrogateescape"))}
    results = []
    for t in a.targets:
        e = entries[t.replace("/", "\\")]
        # cmake -E vs_link_exe ... -- link.exe <options> && <post-build steps,
        # which write into the build directory: left out>
        command = e["command"].split(" -- ", 1)[1].split(" && ", 1)[0]
        if not re.match(r"\S*link\.exe ", command, re.I):
            raise SystemExit(f"{t}: not a link.exe command: {command[:200]}")
        target = os.path.join(os.path.abspath(a.out_dir), os.path.splitext(os.path.basename(t))[0])

        def place(m):
            return f"{m.group(1)}{target}{m.group(3)}"
        command = re.sub(r"(/out:)(\S+?)(\.exe|\.dll)", place, command, flags=re.I)
        command = re.sub(r"(/pdb:)(\S+?)(\.pdb)", place, command, flags=re.I)
        command = re.sub(r"(/implib:)(\S+?)(\.lib)", place, command, flags=re.I)
        command += " " + " ".join(a.link_opt)
        for ext in (".exe", ".dll", ".pdb", ".ilk"):
            if os.path.exists(target + ext):
                os.remove(target + ext)
        script = target + ".link.cmd"
        with open(script, "w") as f:
            f.write(f"@cd /d {os.path.abspath(a.build_dir)}\n{command}\n")
        with open(target + ".log", "w") as log:
            seconds, rc, peak, job_peak = run_in_job(["cmd", "/c", script], env, stdout=log)
        image = next((target + x for x in (".exe", ".dll") if os.path.exists(target + x)), None)
        r = {"target": t, "options": a.link_opt, "seconds": round(seconds, 2), "exit": rc,
             "peak_process_commit": peak, "peak_job_commit": job_peak,
             "exe_bytes": os.path.getsize(image) if image else 0,
             "pdb_bytes": os.path.getsize(target + ".pdb") if os.path.exists(target + ".pdb") else 0}
        print(json.dumps(r), flush=True)
        results.append(r)
    if a.json:
        with open(a.json, "w") as f:
            json.dump(results, f, indent=1)


def link_noinc(a):
    """Links programs the build couldn't link (LNK1140) in place, with the
    build's own command (CMake's manifest and post-build steps included)
    and /INCREMENTAL:NO, so that their tests can run (`test --lit-only`).
    Ninja still counts them as not built."""
    env = vs_env()
    for t in a.targets:
        command = subprocess.run([tool("ninja", env), "-C", a.build_dir, "-t", "commands", "-s",
                                  t], env=env, capture_output=True, text=True,
                                 check=True).stdout.strip().splitlines()[-1]
        rsp = re.search(r"@(\S+\.rsp)", command)
        if rsp and not os.path.exists(os.path.join(a.build_dir, rsp.group(1))):
            # Ninja keeps a response file only after a failed step: write it.
            e = [x for x in json.loads(subprocess.run(
                [tool("ninja", env), "-C", a.build_dir, "-t", "compdb", "-x"], env=env,
                capture_output=True, check=True).stdout.decode("utf-8", "surrogateescape"))
                 if x["output"].replace("/", "\\") == t.replace("/", "\\")][0]["command"]
            before, _, after = command.partition(rsp.group(0))
            inline = e[len(before):len(e) - len(after)]
            with open(os.path.join(a.build_dir, rsp.group(1)), "w") as f:
                f.write(inline)
        command, n = re.subn(r"/INCREMENTAL(?=\s)", "/INCREMENTAL:NO", command)
        if n != 1:
            raise SystemExit(f"{t}: /INCREMENTAL not found once in {command[:300]}")
        script = os.path.join(a.build_dir, os.path.basename(t) + ".noinc.cmd")
        with open(script, "w") as f:
            f.write(f"@cd /d {os.path.abspath(a.build_dir)}\n{command}\n")
        seconds, rc, peak, _ = run_in_job(["cmd", "/c", script], env, timeout=3600)
        print(json.dumps({"target": t, "seconds": round(seconds, 2), "exit": rc,
                          "peak_process_commit": peak}), flush=True)
        if rc:
            raise SystemExit(f"{t}: exit {rc}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    ln = sub.add_parser("link-noinc")
    ln.add_argument("build_dir")
    ln.add_argument("targets", nargs="+")
    rl = sub.add_parser("relink")
    rl.add_argument("build_dir")
    rl.add_argument("out_dir")
    rl.add_argument("targets", nargs="+")
    rl.add_argument("--link-opt", action="append", default=[],
                    help="another link.exe option, as --link-opt=/INCREMENTAL:NO")
    rl.add_argument("--json")
    c = sub.add_parser("configure")
    c.add_argument("mode", choices=MODES)
    c.add_argument("build_dir")
    c.add_argument("--linker", choices=["link", "lld"], default="link")
    c.add_argument("--turbolib", default=TURBOLIB)
    c.add_argument("--link-jobs", type=int, default=3)
    cl = sub.add_parser("clone")
    cl.add_argument("source")
    cl.add_argument("build_dir")
    rh = sub.add_parser("rehash")
    rh.add_argument("source")
    rh.add_argument("build_dir")
    b = sub.add_parser("build")
    b.add_argument("mode", choices=MODES)
    b.add_argument("build_dir")
    b.add_argument("-j", "--jobs", type=int, default=os.cpu_count())
    b.add_argument("-k", "--keep-going", action="store_true")
    b.add_argument("--timeout", type=float, default=8 * 3600, help="seconds (default 8 hours)")
    b.add_argument("targets", nargs="*")
    t = sub.add_parser("test")
    t.add_argument("mode", choices=MODES)
    t.add_argument("build_dir")
    t.add_argument("results")
    t.add_argument("suites", nargs="*", default=["check-llvm", "check-clang", "check-lld"])
    t.add_argument("-j", "--jobs", type=int, default=os.cpu_count())
    t.add_argument("--timeout", type=float, default=12 * 3600,
                   help="seconds for each suite (default 12 hours)")
    t.add_argument("--lit-opt", dest="lit_opts", action="append", default=[],
                   help="more of lit's options, as --lit-opt=--filter-out=REGEX (LIT_OPTS is "
                        "split as a POSIX shell would)")
    t.add_argument("--lit-only", action="store_true",
                   help="run the suite's lit command without building its dependencies (for "
                        "a build whose programs were linked by hand)")
    t.add_argument("--filter-out-file",
                   help="leave out the tests matching the file's regexes (one a line; see "
                        "clang-slow.re)")
    s = sub.add_parser("stats")
    s.add_argument("build_dir")
    s.add_argument("--json")
    s.add_argument("--sizes")
    m = sub.add_parser("measure")
    m.add_argument("mode", choices=MODES)
    m.add_argument("build_dir")
    m.add_argument("targets", nargs="+")
    m.add_argument("--json")
    k = sub.add_parser("compare")
    k.add_argument("base")
    k.add_argument("other")
    r = sub.add_parser("summary")
    r.add_argument("xml", nargs="+")
    a = ap.parse_args()
    if a.cmd == "rehash":
        rehash(os.path.abspath(a.source), os.path.abspath(a.build_dir), vs_env())
        return
    if a.cmd == "summary":
        for x in a.xml:
            o = outcomes(x)
            print(f"{x}: {summary(x)}")
            for t, v in sorted(o.items()):
                if v == "fail":
                    print(f"  FAIL {t}")
        return
    {"configure": configure, "clone": clone, "build": build, "relink": relink, "link-noinc": link_noinc, "test": test,
     "stats": stats,
     "measure": measure, "compare": compare}[a.cmd](a)


if __name__ == "__main__":
    main()
