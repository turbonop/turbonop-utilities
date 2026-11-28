"""Which records lib.exe indexes: features of indexed vs not-indexed records."""
import glob, os, collections, sys
from il import sections
import walk as W

STOR_F = {0x03: 'static', 0x04: 'declared', 0x05: 'defined', 0x09: 'dllexport', 0x0a: 'dllimport'}
STOR_D = {1: 'defined', 2: 'declared', 3: 'common', 4: 'static', 7: 'dllexport', 8: 'dllimport'}


def recs(path):
    g = sections(path)['.cil$gl']
    out = []; W.walk(g, out)
    refd = collections.Counter()
    for a, k, v in out:
        if k == 'fbody':
            for i, how in v[14]:
                refd[int(i, 16)] += 1
        elif k.startswith('data') and v[10]:
            for i, how in v[10]:
                refd[int(i, 16)] += 1
    res = []
    for a, k, v in out:
        if k in ('fbody', 'fdecl4', 'fdecl10'):
            attrs = bytes.fromhex(v[4])
            stor = STOR_F.get(attrs[1], '?%x' % attrs[1])
            res.append(dict(kind='fn', tag=k, id=int(v[0], 16), flags=v[1], name=v[2], stor=stor,
                            f0=v[5]['f0'], f1=v[5]['f1'], k=v[5]['kind'], refd=refd[int(v[0], 16)]))
        elif k.startswith('data'):
            attrs = bytes.fromhex(v[5])
            stor = STOR_D.get(attrs[3], '?%x' % attrs[3])
            res.append(dict(kind='data', tag=k, id=int(v[0], 16), flags=v[1], name=v[2], stor=stor,
                            f0=v[7]['f0'], f1=v[7]['f1'], k=v[7]['kind'], refd=refd[int(v[0], 16)]))
    return res


if __name__ == '__main__':
    files = [f for f in (['fx.obj', 'fx86.obj'] + glob.glob('w/*.obj') + glob.glob('corpus/*.obj') + glob.glob('corpus/gen/*.obj')) if os.path.exists(f + '.idx')]
    feat = collections.Counter()
    for f in files:
        idx = set(open(f + '.idx').read().split())
        x86 = os.path.basename(f).startswith('x86-') or 'fx86' in f
        for r in recs(f):
            n = r['name']
            inn = n in idx or ('_' + n) in idx or ('@' + n) in idx or any(i.startswith(('_' + n + '@', n + '@')) for i in [])
            key = (r['kind'], r['stor'], 'f0=%02x' % (r['f0'] & 0xf0 if r['kind'] == 'fn' else r['f0']), 'f1=%02x' % r['f1'], 'flags=%02x' % r['flags'], 'refd' if r['refd'] else 'unref', 'IDX' if inn else 'no')
            feat[key] += 1
    for k, v in sorted(feat.items()):
        print(v, k)
