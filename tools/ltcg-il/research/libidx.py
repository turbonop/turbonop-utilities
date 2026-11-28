"""For each corpus object, lib.exe's symbol index of a library of it
(dumpbin /linkermember:2), cached as <obj>.idx."""
import concurrent.futures as cf, glob, os, shutil, subprocess, sys, ctypes
ctypes.windll.kernel32.SetPriorityClass(ctypes.windll.kernel32.GetCurrentProcess(), 0x40)
here = os.path.dirname(os.path.abspath(__file__))


def env_for(obj):
    b = os.path.basename(obj)
    f = 'env86.txt' if b.startswith('x86-') or 'fx86' in obj else 'enva64.txt' if b.startswith('a64-') else 'env64.txt'
    env = {}
    for line in open(os.path.join(here, f), encoding='mbcs', errors='replace'):
        k, _, v = line.rstrip('\r\n').partition('=')
        if k:
            env[k] = v
    return env


def index(obj):
    out = obj + '.idx'
    if os.path.exists(out):
        return out
    env = env_for(obj)
    path = next(v for k, v in env.items() if k.lower() == 'path')
    lib = shutil.which('lib', path=path); dumpbin = shutil.which('dumpbin', path=path)
    tmp = obj + '.tmp.lib'
    r = subprocess.run([lib, '/nologo', '/out:' + tmp, obj], env=env, capture_output=True, stdin=subprocess.DEVNULL)
    if r.returncode:
        return None
    r = subprocess.run([dumpbin, '/nologo', '/linkermember:2', tmp], env=env, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    os.remove(tmp)
    text = r.stdout.split('public symbols', 1)[1] if 'public symbols' in r.stdout else ''
    names = []
    for l in text.splitlines():
        t = l.split()
        if len(t) == 2:
            names.append(t[1])
    open(out, 'w').write('\n'.join(names) + '\n')
    return out


if __name__ == '__main__':
    objs = sys.argv[1:] or (['fx.obj', 'fx86.obj'] + glob.glob('w/*.obj') + glob.glob('corpus/*.obj') + glob.glob('corpus/gen/*.obj'))
    with cf.ThreadPoolExecutor(os.cpu_count()) as ex:
        print(sum(1 for x in ex.map(index, objs) if x))
