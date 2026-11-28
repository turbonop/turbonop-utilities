import glob, re, collections, sys
from il import sections
import walk as W, sy as S, ini as I
files = sys.argv[1:] or (['fx.obj'] + glob.glob('w/*.obj') + glob.glob('corpus/*.obj') + glob.glob('corpus/gen/*.obj'))
bad = collections.Counter(); stats = collections.Counter()
PRO = re.compile(rb'\x4f\x1f\x80....\x4f\x20\x80\xfe\x00', re.S)
for f in files:
    s = sections(f)
    g, ex, sy, ini = s['.cil$gl'], s.get('.cil$ex', b''), s.get('.cil$sy', b''), s.get('.cil$in', b'')
    go = []; W.walk(g, go)
    so = []; nfn = S.walk(sy, so)
    io = []; I.walk(ini, io)
    starts = [m.start() for m in PRO.finditer(ex)]
    systarts = [a for a, k, v in so if k == 'FN']
    if len(starts) != nfn: bad['units!=systreams'] += 1; print(f, len(starts), nfn)
    fb = [v for a, k, v in go if k == 'fbody']
    if not all(int(v[9], 16) in set(starts) for v in fb): bad['il not unit start'] += 1
    if not all(int(v[10], 16) in set(systarts) for v in fb): bad['sy not stream start'] += 1
    pair = dict(zip(starts, systarts))
    if not all(pair.get(int(v[9], 16)) == int(v[10], 16) for v in fb): bad['unit/sy order mismatch'] += 1
    inited = set()
    for a, k, v in go:
        if k.startswith('data') and v[7]['f0'] & 0x80: inited.add(int(v[0], 16))
    statics = set(int(v[1], 16) for a, k, v in so if k == 'static')
    begins = set(int(v[0], 16) for a, k, v in io if k == 'begin')
    datas = set(int(v[0], 16) for a, k, v in go if k.startswith('data'))
    if not inited <= begins: bad['inited without initializer'] += 1; print(f, 'missing', [hex(x) for x in sorted(inited - begins)][:5])
    if not begins <= datas | statics:
        bad['initializer of unknown'] += 1
        if '-v' in sys.argv or len(files) < 5: print(f, 'unknown', [hex(x) for x in sorted(begins - datas - statics)][:5])
    stats['files'] += 1; stats['records'] += len(go); stats['bodies'] += len(fb); stats['units'] += len(starts)
    for a, k, v in io:
        if k == 'scalar': stats['scalar kind %d' % v[0]] += 1
        if k == 'begin' and v[1]: stats['begin z nonzero'] += 1
print(bad); print(stats)
