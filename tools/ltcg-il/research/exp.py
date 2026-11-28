"""Compiles small sources with /GL and shows their .cil$gl records (after the
main file's record, up to the predefined namespaces' first record) and other
sections.

usage: python exp.py <name> [-s sec,...] [-o cl-options] -- 'source' ['source2'...]
Each source is compiled as <name>N.cpp (or .c with -c).
"""
import os, subprocess, sys
from il import sections, hexdump

here = os.path.dirname(os.path.abspath(__file__))
work = os.path.join(here, 'w')
os.makedirs(work, exist_ok=True)


def build(name, src, opts=(), ext='.cpp', x86=False):
    path = os.path.join(work, name + ext)
    with open(path, 'w') as f:
        f.write(src + '\n')
    obj = os.path.join(work, name + '.obj')
    cmd = [sys.executable, os.path.join(here, 'msvc.py')] + (['-x86'] if x86 else []) + [
        'cl', '/nologo', '/c', '/O1', '/GL', '/GS-', '/Brepro', *opts, path, '/Fo' + obj]
    r = subprocess.run(cmd, cwd=work, capture_output=True, text=True)
    if r.returncode:
        print(r.stdout, r.stderr)
        raise SystemExit(1)
    return obj


def region(g, name):
    key = (name).encode() + b'\0'
    at = g.index(key) + len(key)
    n = g[at]
    at += 2 + n
    end = g.find(b'\x1cstd\0', at)
    if end < 0:
        end = g.find(b'\x1c__vc_attributes\0', at)
    if end < 0:
        end = len(g)
    return at, end


if __name__ == '__main__':
    a = sys.argv[1:]
    name = a.pop(0)
    secs = ['.cil$gl']
    opts = []
    ext = '.cpp'
    x86 = False
    while a and a[0] != '--':
        o = a.pop(0)
        if o == '-s':
            secs = a.pop(0).split(',')
        elif o == '-o':
            opts += a.pop(0).split()
        elif o == '-c':
            ext = '.c'
        elif o == '-x86':
            x86 = True
    a.pop(0)
    for i, src in enumerate(a):
        n = '%s%d' % (name, i)
        obj = build(n, src, opts, ext, x86)
        s = sections(obj)
        print('#### %s: %s' % (n, src.replace('\n', ' ')))
        for sec in secs:
            d = s.get(sec, b'')
            if sec == '.cil$gl':
                at, end = region(d, n + ext)
                print('== %s [%x..%x of %x]' % (sec, at, end, len(d)))
                hexdump(d[at:end], at)
            else:
                print('== %s %x' % (sec, len(d)))
                hexdump(d)
