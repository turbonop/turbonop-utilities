"""Measures archivers on a Ninja build's own archiver commands: time, CPU,
and peak memory, one library at a time or many at once (as a build runs
them). Windows only (it reads the counters through the Win32 API).

    python perflib.py single <build dir> <tool>... [--largest N] [--rounds R]
                      [--libs NAME,...] [--json FILE]
    python perflib.py parallel <build dir> <tool>... [--largest N] [-j K]
                      [--rounds R] [--json FILE]

A tool is `label=path`, optionally `;NAME=VALUE` after it for an
environment variable (TURBOLIB_OPTIONS), as in timelib.py; the program is
run on each library's command line in the archiver's place (for turbolib's
`lib` personality, a copy of turbolib.exe named lib.exe).

`single` runs each library with each tool, the tools interleaved round by
round, and reports each one's median wall time, user and kernel CPU time,
peak working set, and peak commit (private bytes; the inputs' mappings
are in the working set, not in the commit).

`parallel` runs the libraries (all of the build's, by default) with K at a
time, largest first (as a scheduler that knows them would; Ninja's order is
the build's), each tool in its own job object, and reports the total wall
time, the CPU time of every process, the job's peak commit (all processes
at once), and the highest sum of the processes' working sets sampled every
50 ms.

Inputs are read once first, so the file cache is warm. Outputs go to a
scratch directory and are deleted. Every wait has a timeout.
"""
import argparse
import ctypes
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from ctypes import wintypes

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import relib  # noqa: E402

TIMEOUT = 900

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
psapi = ctypes.WinDLL("psapi", use_last_error=True)


class PMC(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
        ("PrivateUsage", ctypes.c_size_t),
    ]


class IOC(ctypes.Structure):
    _fields_ = [(n, ctypes.c_ulonglong) for n in (
        "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
        "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]


class BASIC_LIMIT(ctypes.Structure):
    _fields_ = [
        ("PerProcessUserTimeLimit", ctypes.c_longlong),
        ("PerJobUserTimeLimit", ctypes.c_longlong),
        ("LimitFlags", wintypes.DWORD),
        ("MinimumWorkingSetSize", ctypes.c_size_t),
        ("MaximumWorkingSetSize", ctypes.c_size_t),
        ("ActiveProcessLimit", wintypes.DWORD),
        ("Affinity", ctypes.c_size_t),
        ("PriorityClass", wintypes.DWORD),
        ("SchedulingClass", wintypes.DWORD),
    ]


class EXT_LIMIT(ctypes.Structure):
    _fields_ = [
        ("BasicLimitInformation", BASIC_LIMIT),
        ("IoInfo", IOC),
        ("ProcessMemoryLimit", ctypes.c_size_t),
        ("JobMemoryLimit", ctypes.c_size_t),
        ("PeakProcessMemoryUsed", ctypes.c_size_t),
        ("PeakJobMemoryUsed", ctypes.c_size_t),
    ]


class BASIC_ACCOUNTING(ctypes.Structure):
    _fields_ = [
        ("TotalUserTime", ctypes.c_longlong),
        ("TotalKernelTime", ctypes.c_longlong),
        ("ThisPeriodTotalUserTime", ctypes.c_longlong),
        ("ThisPeriodTotalKernelTime", ctypes.c_longlong),
        ("TotalPageFaultCount", wintypes.DWORD),
        ("TotalProcesses", wintypes.DWORD),
        ("ActiveProcesses", wintypes.DWORD),
        ("TotalTerminatedProcesses", wintypes.DWORD),
    ]


k32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
k32.OpenProcess.restype = wintypes.HANDLE
k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
k32.CreateJobObjectW.restype = wintypes.HANDLE
k32.GetCurrentProcess.restype = wintypes.HANDLE
k32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
k32.QueryInformationJobObject.argtypes = [
    wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p]
psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(PMC), wintypes.DWORD]


def ft(f):
    return ((f.dwHighDateTime << 32) | f.dwLowDateTime) / 1e7


def process_counters(handle):
    c, e, k, u = (wintypes.FILETIME() for _ in range(4))
    k32.GetProcessTimes(handle, ctypes.byref(c), ctypes.byref(e), ctypes.byref(k), ctypes.byref(u))
    m = PMC()
    m.cb = ctypes.sizeof(PMC)
    psapi.GetProcessMemoryInfo(handle, ctypes.byref(m), m.cb)
    return {"user": ft(u), "kernel": ft(k), "peak_ws": m.PeakWorkingSetSize,
            "peak_commit": m.PeakPagefileUsage, "faults": m.PageFaultCount}


def tools_of(specs):
    tools = []
    for t in specs:
        label, _, rest = t.partition("=") if "=" in t.split(";")[0] else (t, "", t)
        path, _, env = rest.partition(";")
        environ = dict(os.environ)
        for kv in filter(None, env.split(";")):
            k, _, v = kv.partition("=")
            environ[k] = v
        tools.append((label or os.path.basename(path), path, environ))
    return tools


def libraries(build_dir, largest, names=None):
    rules = relib.read_rules(os.path.join(build_dir, "CMakeFiles", "rules.ninja"))
    libs = [l for l in relib.read_libraries(build_dir, rules)
            if os.path.exists(os.path.join(build_dir, l["out"]))]
    libs.sort(key=lambda l: -os.path.getsize(os.path.join(build_dir, l["out"])))
    if names:
        want = set(names)
        libs = [l for l in libs if os.path.basename(l["out"]) in want]
    elif largest:
        libs = libs[:largest]
    return rules, libs


def warm(build_dir, libs):
    for l in libs:
        for p in l["in"]:
            full = os.path.join(build_dir, p)
            if os.path.exists(full):
                with open(full, "rb") as f:
                    while f.read(1 << 24):
                        pass


ntdll = ctypes.WinDLL("ntdll")
ntdll.NtResumeProcess.argtypes = [wintypes.HANDLE]
CREATE_SUSPENDED = 0x4


def new_job():
    return k32.CreateJobObjectW(None, None)


def start(build_dir, rules, lib, path, environ, out, job=None):
    """The archiver started on `lib`'s command, in `job` (from its first
    instruction: it starts suspended), writing `out`."""
    rsp = out + ".rsp"
    args, content = relib.archiver_args(lib, rules, out, rsp)
    if content is not None:
        with open(rsp, "w", encoding="utf-8") as f:
            f.write(content)
    p = subprocess.Popen([path] + args, cwd=build_dir, env=environ,
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                         creationflags=CREATE_SUSPENDED if job else 0)
    if job:
        if not k32.AssignProcessToJobObject(job, int(p._handle)):
            p.kill()
            sys.exit(f"AssignProcessToJobObject: {ctypes.get_last_error()}")
        ntdll.NtResumeProcess(int(p._handle))
    return p


k32.QueryInformationJobObject.restype = wintypes.BOOL


class PIDLIST(ctypes.Structure):
    _fields_ = [("Assigned", wintypes.DWORD), ("Listed", wintypes.DWORD),
                ("Ids", ctypes.c_size_t * 256)]


def job_ws(job):
    """The sum of the working sets of the job's processes now."""
    ids = PIDLIST()
    if not k32.QueryInformationJobObject(job, 3, ctypes.byref(ids), ctypes.sizeof(ids), None):
        return 0
    total = 0
    for pid in ids.Ids[:ids.Listed]:
        h = k32.OpenProcess(0x1000 | 0x0400, False, pid)  # limited query, VM read
        if not h:
            continue
        m = PMC()
        m.cb = ctypes.sizeof(PMC)
        if psapi.GetProcessMemoryInfo(h, ctypes.byref(m), m.cb):
            total += m.WorkingSetSize
        k32.CloseHandle(h)
    return total


def job_counters(job):
    acc = BASIC_ACCOUNTING()
    k32.QueryInformationJobObject(job, 1, ctypes.byref(acc), ctypes.sizeof(acc), None)
    ext = EXT_LIMIT()
    k32.QueryInformationJobObject(job, 9, ctypes.byref(ext), ctypes.sizeof(ext), None)
    return {"user": acc.TotalUserTime / 1e7, "kernel": acc.TotalKernelTime / 1e7,
            "faults": acc.TotalPageFaultCount,
            "peak_commit": ext.PeakJobMemoryUsed,
            "peak_process_commit": ext.PeakProcessMemoryUsed}


class Sampler:
    """The highest sum of a job's working sets, sampled every `every` s."""

    def __init__(self, job, every=0.02):
        self.peak = 0
        self.stop = threading.Event()

        def run():
            while not self.stop.wait(every):
                self.peak = max(self.peak, job_ws(job))
        self.thread = threading.Thread(target=run, daemon=True)
        self.thread.start()

    def done(self):
        self.stop.set()
        self.thread.join(timeout=5)
        return self.peak


def finish(p, label, lib):
    try:
        output, _ = p.communicate(timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        p.kill()
        p.communicate(timeout=60)
        sys.exit(f"{label} timed out on {lib['out']}")
    if p.returncode != 0:
        sys.exit(f"{label} failed on {lib['out']}: {output.decode(errors='replace')[:500]}")
    return output.decode(errors="replace")


def single(a):
    rules, libs = libraries(a.build_dir, a.largest, a.libs and a.libs.split(","))
    tools = tools_of(a.tools)
    warm(a.build_dir, libs)
    scratch = tempfile.mkdtemp(prefix="perflib-")
    results = {label: [] for label, _, _ in tools}
    try:
        for l in libs:
            runs = {label: [] for label, _, _ in tools}
            for _ in range(a.rounds):
                for label, path, environ in tools:
                    out = os.path.join(scratch, "out.lib")
                    job = new_job()
                    t0 = time.perf_counter()
                    p = start(a.build_dir, rules, l, path, environ, out, job)
                    sampler = Sampler(job)
                    text = finish(p, label, l)
                    wall = time.perf_counter() - t0
                    c = job_counters(job)
                    # The process's own peak (exact) where there is one
                    # process; lib.exe runs link.exe, sampled.
                    own = process_counters(int(p._handle))
                    c["peak_ws"] = max(own["peak_ws"], sampler.done())
                    k32.CloseHandle(job)
                    c["wall"] = wall
                    c["stats"] = [x for x in text.splitlines() if x.startswith("turbolib: ")
                                  and " s:" in x]
                    runs[label].append(c)
                    os.remove(out)
            for label, _, _ in tools:
                rs = runs[label]
                med = {k: statistics.median(r[k] for r in rs)
                       for k in ("wall", "user", "kernel", "peak_ws", "peak_commit", "faults")}
                med["lib"] = os.path.basename(l["out"])
                med["size"] = os.path.getsize(os.path.join(a.build_dir, l["out"]))
                med["stats"] = min(rs, key=lambda r: r["wall"])["stats"]
                results[label].append(med)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    mb = 1 << 20
    print(f"{len(libs)} libraries, median of {a.rounds} interleaved rounds, warm cache")
    print(f"{'library':34} {'tool':14} {'wall':>7} {'user':>7} {'kern':>6} {'peakWS':>8} {'commit':>8}")
    for i, l in enumerate(libs):
        for label, _, _ in tools:
            r = results[label][i]
            print(f"{r['lib'][:34]:34} {label[:14]:14} {r['wall']:7.2f} {r['user']:7.2f} "
                  f"{r['kernel']:6.2f} {r['peak_ws'] / mb:7.0f}M {r['peak_commit'] / mb:7.0f}M")
            for s in r["stats"]:
                print(f"{'':34}   {s}")
    print("totals (sum over libraries; memory: the largest)")
    for label, _, _ in tools:
        rs = results[label]
        print(f"{label[:48]:48} wall {sum(r['wall'] for r in rs):7.2f} cpu "
              f"{sum(r['user'] + r['kernel'] for r in rs):7.2f} peakWS "
              f"{max(r['peak_ws'] for r in rs) / mb:6.0f}M commit "
              f"{max(r['peak_commit'] for r in rs) / mb:6.0f}M")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(results, f, indent=1)


def pool(a):
    """One tool's run over the libraries, its processes in one job object."""
    rules, libs = libraries(a.build_dir, a.largest)
    (label, path, environ), = tools_of(a.tools)
    job = new_job()
    scratch = tempfile.mkdtemp(prefix="perflib-")
    running = {}
    queue = list(range(len(libs)))
    t0 = time.perf_counter()
    sampler = Sampler(job, 0.05)
    try:
        while queue or running:
            while queue and len(running) < a.j:
                i = queue.pop(0)
                out = os.path.join(scratch, f"{i}.lib")
                running[i] = start(a.build_dir, rules, libs[i], path, environ, out, job)
            done = None
            deadline = time.perf_counter() + TIMEOUT
            while done is None:
                for i, p in list(running.items()):
                    if p.poll() is not None:
                        done = i
                        break
                else:
                    if time.perf_counter() > deadline:
                        sys.exit("a library timed out")
                    time.sleep(0.005)
            finish(running.pop(done), label, libs[done])
            out = os.path.join(scratch, f"{done}.lib")
            os.remove(out)
            if os.path.exists(out + ".rsp"):
                os.remove(out + ".rsp")
        wall = time.perf_counter() - t0
    finally:
        peak_ws = sampler.done()
        shutil.rmtree(scratch, ignore_errors=True)
    c = job_counters(job)
    print(json.dumps({"label": label, "libs": len(libs), "j": a.j, "wall": wall,
                      "user": c["user"], "kernel": c["kernel"],
                      "peak_job_commit": c["peak_commit"],
                      "peak_process_commit": c["peak_process_commit"],
                      "peak_ws_sum": peak_ws}))


def timeline(a):
    """One run of one tool on one library: its private bytes and working
    set every 10 ms, with the stages `--stats` reports."""
    rules, libs = libraries(a.build_dir, 0, a.libs.split(","))
    (label, path, environ), = tools_of(a.tools)
    scratch = tempfile.mkdtemp(prefix="perflib-")
    out = os.path.join(scratch, "out.lib")
    samples = []
    try:
        t0 = time.perf_counter()
        p = start(a.build_dir, rules, libs[0], path, environ, out)
        h = int(p._handle)
        while p.poll() is None and time.perf_counter() - t0 < TIMEOUT:
            m = PMC()
            m.cb = ctypes.sizeof(PMC)
            if psapi.GetProcessMemoryInfo(h, ctypes.byref(m), m.cb):
                samples.append((time.perf_counter() - t0, m.PrivateUsage, m.WorkingSetSize))
            time.sleep(0.01)
        text = finish(p, label, libs[0])
        c = process_counters(h)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    mb = 1 << 20
    print(text)
    print(f"peak commit {c['peak_commit'] / mb:.0f} MiB, peak ws {c['peak_ws'] / mb:.0f} MiB")
    step = max(1, len(samples) // 60)
    for t, priv, ws in samples[::step]:
        print(f"{t:6.3f} s  private {priv / mb:6.0f} MiB  ws {ws / mb:6.0f} MiB  "
              + "#" * int(priv / mb / 50))


def identical(a):
    """Each library made by the first tool and by each other, with /Brepro:
    the same bytes? (Up to -j libraries at once.)"""
    rules, libs = libraries(a.build_dir, a.largest, a.libs and a.libs.split(","))
    tools = tools_of(a.tools)
    scratch = tempfile.mkdtemp(prefix="perflib-")
    bad = 0

    def one(i):
        lib = libs[i]
        outs = []
        for k, (label, path, environ) in enumerate(tools):
            out = os.path.join(scratch, f"{i}.{k}.lib")
            rsp = out + ".rsp"
            args, content = relib.archiver_args(lib, rules, out, rsp)
            if content is not None:
                with open(rsp, "w", encoding="utf-8") as f:
                    f.write(content)
            p = subprocess.run([path, "/Brepro"] + args, cwd=a.build_dir, env=environ,
                               capture_output=True, timeout=TIMEOUT)
            if p.returncode != 0:
                return f"{label} failed on {lib['out']}: {p.stdout[:300]!r}"
            with open(out, "rb") as f:
                outs.append(f.read())
            os.remove(out)
            if content is not None:
                os.remove(rsp)
        for k in range(1, len(tools)):
            if outs[k] != outs[0]:
                return f"{os.path.basename(lib['out'])}: {tools[k][0]} differs from {tools[0][0]}"
        return None

    from concurrent.futures import ThreadPoolExecutor
    try:
        with ThreadPoolExecutor(a.j) as ex:
            for i, r in enumerate(ex.map(one, range(len(libs)))):
                if r:
                    bad += 1
                    print(r, flush=True)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    print(f"{len(libs)} libraries, {bad} differ")
    sys.exit(1 if bad else 0)


def parallel(a):
    rules, libs = libraries(a.build_dir, a.largest)
    warm(a.build_dir, libs)
    tools = a.tools
    results = {t: [] for t in tools}
    for _ in range(a.rounds):
        for t in tools:
            args = [sys.executable, os.path.abspath(__file__), "_pool", a.build_dir, t,
                    "-j", str(a.j)]
            if a.largest:
                args += ["--largest", str(a.largest)]
            p = subprocess.run(args, capture_output=True, timeout=TIMEOUT * 4)
            if p.returncode != 0:
                sys.exit(p.stdout.decode() + p.stderr.decode())
            results[t].append(json.loads(p.stdout.decode().strip().splitlines()[-1]))
    gb = 1 << 30
    print(f"{len(libs)} libraries, -j {a.j}, median of {a.rounds} interleaved rounds")
    for t in tools:
        rs = results[t]
        med = {k: statistics.median(r[k] for r in rs)
               for k in ("wall", "user", "kernel", "peak_job_commit", "peak_ws_sum",
                         "peak_process_commit")}
        print(f"{rs[0]['label'][:40]:40} wall {med['wall']:7.2f} s  cpu "
              f"{med['user'] + med['kernel']:8.2f} s (kernel {med['kernel']:.2f})  "
              f"commit {med['peak_job_commit'] / gb:5.2f} G  ws {med['peak_ws_sum'] / gb:5.2f} G  "
              f"(one process at most {med['peak_process_commit'] / gb:4.2f} G)"
              f"  walls {' '.join(f'{r['wall']:.2f}' for r in rs)}")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(results, f, indent=1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("command", choices=["single", "parallel", "timeline", "identical", "_pool"])
    ap.add_argument("build_dir")
    ap.add_argument("tools", nargs="+")
    ap.add_argument("--largest", type=int, default=0)
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--libs")
    ap.add_argument("-j", type=int, default=os.cpu_count())
    ap.add_argument("--json")
    a = ap.parse_args()
    {"single": single, "parallel": parallel, "timeline": timeline, "identical": identical, "_pool": pool}[a.command](a)


if __name__ == "__main__":
    main()
