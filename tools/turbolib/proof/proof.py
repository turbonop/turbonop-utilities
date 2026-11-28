"""Builds LLVM, clang, and lld from scratch with lib.exe or turbolib as the
archiver, runs their test suites, and measures the builds: the proof that
turbolib can stand in for lib.exe on a large /Z7 project.

    python proof.py configure <mode> <build dir> [--linker link|lld]
    python proof.py build <mode> <build dir> [-j N] [target...]
    python proof.py test <mode> <build dir> <results dir> [suite...]
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

`build` runs Ninja in the Visual Studio environment (vcvars64.bat) and
appends the wall time to <build dir>\\proof-times.json. `test` runs the
check targets (check-llvm, check-clang, check-lld by default) with lit's
xunit output in <results dir>. `stats` reads .ninja_log: every step's time,
the archiver steps' (static libraries) total, the big programs' links, and
the libraries' sizes. `measure` deletes each target's image, PDB, and
incremental-link file and relinks it with Ninja (the build's own command,
/INCREMENTAL as configured) inside a job object, reporting the time and the
peak commit of the largest process. `compare` lists the tests that pass in
the first results and not in the second.
"""
import argparse
import ctypes
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
        args.append("-DCMAKE_AR=" + turbolib_copy(a.build_dir, a.mode, a.turbolib))
    if a.mode == "fold-types":
        args.append("-DCMAKE_STATIC_LINKER_FLAGS=/machine:x64 /TURBOLIB:fold=identical "
                    "/TURBOLIB:precomp=library")
    if a.linker == "lld":
        args.append("-DCMAKE_LINKER=" + LLD)
    print(" ".join(args), flush=True)
    sys.exit(subprocess.run(args, env=env).returncode)


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
    rc = subprocess.run(args, env=env).returncode
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
        e["LIT_OPTS"] = f"--xunit-xml-output={xml.replace(chr(92), '/')} --no-progress-bar -q"
        start = time.time()
        log = os.path.join(a.results, suite + ".log")
        with open(log, "w", encoding="utf-8", errors="replace") as f:
            r = subprocess.run([tool("ninja", env), "-C", a.build_dir, f"-j{a.jobs}", suite], env=e,
                               stdout=f, stderr=subprocess.STDOUT).returncode
        seconds = time.time() - start
        record_time(a.build_dir, {"targets": [suite], "seconds": round(seconds, 1), "exit": r,
                                  "started": time.strftime("%Y-%m-%d %H:%M:%S",
                                                           time.localtime(start))})
        print(f"proof: {suite}: {seconds:.0f} s, exit {r}; {summary(xml)}", flush=True)
        rc = rc or r
    sys.exit(rc)


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


def run_in_job(args, env):
    """Runs a command in a new job: (seconds, exit, peak process commit,
    peak job commit)."""
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreateJobObjectW.restype = wintypes.HANDLE
    job = k32.CreateJobObjectW(None, None)
    start = time.perf_counter()
    # Suspended, so that every child process is born in the job.
    p = subprocess.Popen(args, env=env, creationflags=0x4)  # CREATE_SUSPENDED
    if not k32.AssignProcessToJobObject(wintypes.HANDLE(job), wintypes.HANDLE(int(p._handle))):
        raise SystemExit(f"AssignProcessToJobObject: {ctypes.get_last_error()}")
    resume_main_thread(p.pid)
    rc = p.wait()
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


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("configure")
    c.add_argument("mode", choices=MODES)
    c.add_argument("build_dir")
    c.add_argument("--linker", choices=["link", "lld"], default="link")
    c.add_argument("--turbolib", default=TURBOLIB)
    c.add_argument("--link-jobs", type=int, default=3)
    b = sub.add_parser("build")
    b.add_argument("mode", choices=MODES)
    b.add_argument("build_dir")
    b.add_argument("-j", "--jobs", type=int, default=os.cpu_count())
    b.add_argument("-k", "--keep-going", action="store_true")
    b.add_argument("targets", nargs="*")
    t = sub.add_parser("test")
    t.add_argument("mode", choices=MODES)
    t.add_argument("build_dir")
    t.add_argument("results")
    t.add_argument("suites", nargs="*", default=["check-llvm", "check-clang", "check-lld"])
    t.add_argument("-j", "--jobs", type=int, default=os.cpu_count())
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
    if a.cmd == "summary":
        for x in a.xml:
            o = outcomes(x)
            print(f"{x}: {summary(x)}")
            for t, v in sorted(o.items()):
                if v == "fail":
                    print(f"  FAIL {t}")
        return
    {"configure": configure, "build": build, "test": test, "stats": stats,
     "measure": measure, "compare": compare}[a.cmd](a)


if __name__ == "__main__":
    main()
