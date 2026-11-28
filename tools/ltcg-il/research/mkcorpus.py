"""Compiles the repository's tool sources with /GL for several targets and
option sets, in parallel, into corpus/gen/. Failures are skipped."""
import concurrent.futures as cf, glob, os, subprocess, sys, ctypes
ctypes.windll.kernel32.SetPriorityClass(ctypes.windll.kernel32.GetCurrentProcess(), 0x40)  # idle

here = os.path.dirname(os.path.abspath(__file__))
src_root = os.path.abspath(os.path.join(here, '..', '..'))  # tools/
out = os.path.join(here, 'corpus', 'gen')
os.makedirs(out, exist_ok=True)
skip = ('unix', 'macho', 'elf', 'wasm', 'llvm', 'mingw', 'loongarch', 'mips', 'powerpc', 'riscv', 'rust')
sources = [p for p in glob.glob(src_root + r'\**\*.c*', recursive=True)
           if p.endswith(('.c', '.cpp')) and not any(s in p.lower() for s in skip)]
variants = [
    ('x64', 'env64.txt', ['/O1', '/EHsc', '/std:c++20']),
    ('x64od', 'env64.txt', ['/Od', '/EHsc', '/Zi', '/GS', '/guard:cf']),
    ('x86', 'env86.txt', ['/O2', '/EHsc']),
    ('a64', 'enva64.txt', ['/O2', '/EHsc', '/std:c++17']),
]


def build(job):
    src, (name, envf, opts) = job
    if not os.path.exists(os.path.join(here, envf)):
        return None
    rel = os.path.relpath(src, src_root).replace('\\', '_').replace('/', '_')
    obj = os.path.join(out, '%s-%s.obj' % (name, rel.rsplit('.', 1)[0]))
    env = {}
    for line in open(os.path.join(here, envf), encoding='mbcs', errors='replace'):
        k, _, v = line.rstrip('\r\n').partition('=')
        if k:
            env[k] = v
    import shutil
    path = next(v for k, v in env.items() if k.lower() == 'path')
    cl = shutil.which('cl', path=path)
    r = subprocess.run([cl, '/nologo', '/c', '/GL', '/Brepro', '/I' + os.path.dirname(src), *opts, src, '/Fo' + obj,
                        '/Fd' + obj[:-4] + '.pdb'],
                       env=env, cwd=out, capture_output=True, stdin=subprocess.DEVNULL)
    return obj if r.returncode == 0 else None


jobs = [(s, v) for s in sources for v in variants]
with cf.ThreadPoolExecutor(os.cpu_count()) as ex:
    done = [o for o in ex.map(build, jobs) if o]
print(len(done), 'of', len(jobs))
