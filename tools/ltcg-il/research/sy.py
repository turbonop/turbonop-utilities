"""Experimental walker of .cil$sy: per-function streams of scopes and locals."""
import struct, sys
from il import sections, hexdump
from walk import R, Stop, hx, common, attrs_list


def local_tail(r, agg=False):
    mt = r.u8()
    al = r.u8() if mt & 0x40 else None
    attrs = r.take(4)
    size = r.c32()
    c = common(r, False)
    t = r.c32()
    al2 = r.u8() if agg else None
    attrs_list(r, c)
    return (hex(mt), hx(attrs), size, c, hex(t), al2)


def walk_fn(r, out):
    """One function's stream, from r.p; returns at its end (after 06)."""
    start = r.p
    while True:
        at = r.p
        tag = r.u8()
        if tag == 0x06:
            out.append((at, 'end', None))
            return
        elif tag == 0x03:
            lvl = r.u8(); i = r.id(); kind = r.u8(); name = r.sz(); n = r.u8(); one = r.u8()
            out.append((at, 'scope', (lvl, hex(i), hex(kind), name, n, one)))
        elif tag == 0x0d:
            out.append((at, 'mark', r.u8()))
        elif tag in (0x01, 0x02):
            sc = r.u8(); i = r.id(); fl = r.u8(); name = r.sz()
            out.append((at, 'local%d' % tag, (sc, hex(i), fl, name) + local_tail(r, tag == 2)))
        elif tag == 0x0b:
            z = r.u8(); i = r.id(); name = r.sz(); z2 = r.u8(); t = r.c32(); c = common(r, False); attrs_list(r, c)
            out.append((at, 'type', (z, hex(i), name, z2, hex(t), c)))
        elif tag == 0x08:
            z = r.u8(); i = r.id(); name = r.sz(); t = r.c32(); attrs = r.take(5); v = r.take(attrs[4])
            out.append((at, 'const', (z, hex(i), name, hex(t), hx(attrs), hx(v))))
        elif tag == 0x1a:
            sc = r.u8(); i = r.id(); mt = r.u8(); al = r.u8() if mt & 0x40 else None; tc = r.u8(); size = r.c32()
            c = common(r, False); t = r.c32(); attrs_list(r, c)
            out.append((at, 'anon', (sc, hex(i), hex(mt), al, tc, size, c, hex(t))))
        elif tag == 0x07:
            sc = r.u8(); i = r.id(); name = r.sz(); mt = r.u8(); al = r.u8() if mt & 0x40 else None
            a = r.take(2); size = r.c32(); b = r.take(2); t = r.c32(); g = r.id(); z = r.u8()
            out.append((at, 'static', (sc, hex(i), name, hex(mt), hx(a), size, hx(b), hex(t), hex(g), z)))
        else:
            raise Stop('sy tag %x at %x (fn at %x)' % (tag, at, start))


def walk(sy, out, starts=None):
    r = R(sy, 0)
    nfn = 0
    while r.p < len(sy):
        out.append((r.p, 'FN', nfn))
        walk_fn(r, out)
        nfn += 1
    return nfn


if __name__ == '__main__':
    quiet = '-q' in sys.argv
    for path in [a for a in sys.argv[1:] if a != '-q']:
        sy = sections(path).get('.cil$sy', b'')
        out = []
        try:
            n = walk(sy, out)
            st = None
        except (Stop, IndexError, struct.error, ValueError) as e:
            st = e; n = None
        print('##', path, 'STOPPED: %s' % st if st else 'complete', n)
        show = out if not quiet else out[-8:]
        for at, k, v in show:
            print('%06x %s %s' % (at, k, v))
        if st:
            at = out[-1][0] if out else 0
            hexdump(sy[at:at + 128], at)
