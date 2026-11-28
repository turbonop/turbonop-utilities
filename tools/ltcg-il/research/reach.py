import glob, os, collections, sys
from il import sections
import walk as W
from pubrule import STOR_F, STOR_D

def analyze(path):
    g = sections(path)['.cil$gl']
    out = []; W.walk(g, out)
    recs = {}; edges = collections.defaultdict(list)
    for a, k, v in out:
        if k in ('fbody', 'fdecl4', 'fdecl10'):
            attrs = bytes.fromhex(v[4]); i = int(v[0], 16)
            recs[i] = dict(kind='fn', body=k == 'fbody', name=v[2], stor=STOR_F.get(attrs[1], '?'), f0=v[5]['f0'], flags=v[1], bf=int.from_bytes(bytes.fromhex(v[12])[4:], 'little') if k == 'fbody' else 0)
            if k == 'fbody': edges[i] += [int(x, 16) for x, h in v[14] if h != 0]
            if k == 'fdecl10': edges[i].append(int(v[5]['assoc'], 16))
        elif k.startswith('data'):
            attrs = bytes.fromhex(v[5]); i = int(v[0], 16)
            recs[i] = dict(kind='data', name=v[2], stor=STOR_D.get(attrs[3], '?'), f0=v[7]['f0'], flags=v[1])
            if v[10]: edges[i] += [int(x, 16) for x, h in v[10] if h != 0]
    return recs, edges

def predict(recs, edges, rootrule):
    roots = [i for i, r in recs.items() if rootrule(r)]
    seen = set(roots); st = list(roots)
    while st:
        i = st.pop()
        for j in edges.get(i, ()):
            if j not in seen: seen.add(j); st.append(j)
    return seen

import os as _os
STATIC_ROOTS = _os.environ.get('STATIC_ROOTS') == '1'
def is_root(r):
    if r['kind'] == 'fn':
        if r['stor'] == 'static' and STATIC_ROOTS: return r.get('body') and not (r['f0'] & 0x20)
        return bool(r['bf'] & 0x20)
    if r['stor'] == 'common': return True
    if r['stor'] not in ('defined', 'dllexport', 'static'): return False
    if r['stor'] == 'static' and not STATIC_ROOTS: return False
    return not (r['f0'] & 0x20) or bool(r['f0'] & 0x40)

if __name__ == '__main__':
    files = [f for f in (['fx.obj', 'fx86.obj'] + glob.glob('w/*.obj') + glob.glob('corpus/*.obj') + glob.glob('corpus/gen/*.obj')) if os.path.exists(f + '.idx')]
    tot = collections.Counter(); ex = collections.defaultdict(list)
    for f in files:
        idx = set(open(f + '.idx').read().split())
        recs, edges = analyze(f)
        reach = predict(recs, edges, is_root)
        for i, r in recs.items():
            if r['stor'] not in ('defined', 'dllexport', 'common'): continue
            n = r['name']
            inn = n in idx or ('_' + n) in idx or ('@' + n) in idx or any(x.startswith('_' + n + '@') for x in idx if '@' in n or True) if False else (n in idx or ('_' + n) in idx or ('@' + n) in idx)
            pred = i in reach
            key = ('IDX' if inn else 'no', 'reach' if pred else 'unreach', r['kind'], 'inl' if r['f0'] & 0x20 else '')
            tot[key] += 1
            if inn != pred and len(ex[key]) < 6: ex[key].append((f, n, hex(r['f0']), hex(r['flags'])))
    for k, v in sorted(tot.items()): print(v, k)
    for k, v in ex.items(): print(k); [print('   ', x) for x in v]
