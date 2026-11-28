"""Archives each library again with lib.exe and turbolib (`/REMOVE` of a
member that isn't there, so neither copies it), without /MACHINE and with
/MACHINE:ARM64X, and compares the bytes and messages:

    python rearchive.py <turbolib.exe> <dir>...

Meant for ARM64 libraries (the SDK's um\arm64, ucrt\arm64, MSVC's
lib\arm64). Runs at below-normal priority; results in a scratch directory,
removed at the end. Prints counts and the first 60 differences.
"""
import os, subprocess, sys, tempfile, collections
from concurrent.futures import ThreadPoolExecutor
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from everylib import lib_exe  # noqa: E402
LIB = lib_exe()
T = sys.argv[1]; dirs = sys.argv[2:]
libs = sorted(os.path.join(d, f) for top in dirs for d, _, fs in os.walk(top) for f in fs if f.lower().endswith('.lib'))
scratch = tempfile.mkdtemp(prefix='rearchive-')
env = dict(os.environ); env.pop('TURBOLIB_OPTIONS', None)
LOW = subprocess.BELOW_NORMAL_PRIORITY_CLASS
def one(a):
    i, lib, mach = a
    out = []
    for tool, cmd in (('ms', [LIB]), ('t', [T, 'lib'])):
        o = os.path.join(scratch, f'{i}{mach}.{tool}.lib')
        p = subprocess.run(cmd + ['/nologo', '/Brepro', '/REMOVE:turbolib-none.obj'] + ([f'/MACHINE:{mach}'] if mach else []) + [f'/OUT:{o}', lib],
                           capture_output=True, text=True, errors='replace', env=env, timeout=600, creationflags=LOW)
        msg = [l for l in (p.stdout + p.stderr).splitlines() if 'LNK4014' not in l and l.strip()]
        data = open(o, 'rb').read() if os.path.exists(o) else None
        if os.path.exists(o): os.remove(o)
        out.append((p.returncode, msg, data))
    (r1, m1, d1), (r2, m2, d2) = out
    if r1 != r2: v = f'exit {r1} vs {r2}'
    elif d1 != d2: v = 'bytes differ'
    elif m1 != m2: v = 'messages differ'
    else: v = 'same'
    return lib, mach, v, (m1[:2], m2[:2])
jobs = [(i, l, m) for i, l in enumerate(libs) for m in ('', 'ARM64X')]
res = collections.Counter(); bad = []
with ThreadPoolExecutor(6) as ex:
    for lib, mach, v, msgs in ex.map(one, jobs):
        res[(mach or 'none', v)] += 1
        if v != 'same': bad.append((lib, mach, v, msgs))
os.rmdir(scratch)
print(len(libs), 'libraries')
for k, n in sorted(res.items()): print(' ', k, n)
for b in bad[:60]: print(b)
