"""Experimental walker of .cil$gl: every record in turn, strict."""
import struct, sys
from il import sections, hexdump


class Stop(Exception):
    pass


class R:
    def __init__(s, d, p=0):
        s.d, s.p = d, p

    def u8(s):
        v = s.d[s.p]; s.p += 1; return v

    def u16(s):
        v = struct.unpack_from('<H', s.d, s.p)[0]; s.p += 2; return v

    def u32(s):
        v = struct.unpack_from('<I', s.d, s.p)[0]; s.p += 4; return v

    def id(s):
        w = s.u16()
        if w & 0x8000 == 0:
            return w
        return (w & 0x7fff) | (s.u16() << 15)

    def c16(s):
        b = s.u8()
        if b < 0x80: return b
        if b != 0x80: raise Stop('c16 %x at %x' % (b, s.p - 1))
        return s.u16()

    def c32(s):
        b = s.u8()
        if b < 0x80: return b
        if b != 0x80: raise Stop('c32 %x at %x' % (b, s.p - 1))
        return s.u32()

    def leb(s):
        v = 0; sh = 0
        while True:
            b = s.u8(); v |= (b & 0x7f) << sh; sh += 7
            if b < 0x80: return v

    def sz(s):
        e = s.d.index(0, s.p)
        v = s.d[s.p:e]; s.p = e + 1; return v.decode('latin1')

    def take(s, n):
        v = s.d[s.p:s.p + n]; s.p += n; return v


def hx(b):
    return ' '.join('%02x' % x for x in b)


def common(r, fn):
    """F0 F1 [X] [section] one kind extras."""
    f0 = r.u8(); f1 = r.u8()
    x = r.u16() if f1 & 0x80 else None
    sec = r.id() if f1 & 2 else None
    one = r.u32()
    kind = r.u32()
    at = r.p
    if fn:
        k0 = kind & 0xff
        if k0 & 1:
            x0, n = r.u8(), r.u8()
            for _ in range(n):
                r.id()
        if k0 & 2:
            r.take(2)
        if k0 & 0x74:
            raise Stop('function kind %x at %x' % (kind, r.p))
    else:
        if kind & 0x7f == 2:
            p, q = r.u8(), r.u8()
            if p & 1:
                r.leb()
        elif kind & 0x7f:
            raise Stop('data kind %x at %x' % (kind, r.p))
    return dict(f0=f0, f1=f1, x=x, sec=sec, one=one, kind=kind, ext=hx(r.d[at:r.p]))


def block(r):
    at = r.p
    a = r.take(3)
    n = r.u32()
    extra = b''
    if n:
        extra = r.take(EXTRA.get(n, 0))
    return hx(r.d[at:r.p])


EXTRA = {2: 2}


def attrs_list(r, c):
    if (c['kind'] >> 16) & 1:
        n = r.c16()
        c['attrs'] = [hex(r.id()) for _ in range(n)]


def walk(g, out):
    r = R(g, 30)
    while r.p < len(g):
        at = r.p
        tag = r.u8()
        if tag == 0x18:
            a, b, c = r.c16(), r.c16(), r.c16()
            out.append((at, 'pre', (a, b, c)))
        elif tag == 0x12:
            i = r.id(); path = r.sz(); n = r.u8(); k = r.u8(); r.take(n)
            out.append((at, 'file', path))
        elif tag == 0x0a:
            r.take(2); n = r.u8()
            ds = [(r.u8(), r.sz()) for _ in range(n)]
            out.append((at, 'directives', ds))
            if r.p != len(g):
                raise Stop('directives end %x of %x' % (r.p, len(g)))
            return
        elif tag == 0x09:
            i = r.id(); name = r.sz(); fl = r.u8()
            cls = r.sz()
            ch = r.u32()
            out.append((at, 'section', (hex(i), name, fl, cls, hex(ch))))
        elif tag == 0x1d:
            i = r.id(); n = r.c16(); ns = r.take(n); n = r.c16(); name = r.take(n); z = r.u8()
            args = []
            for _ in range(z):
                k = r.u8(); n = r.c16(); args.append((k, r.take(n)))
            out.append((at, 'attribute', (hex(i), ns, name, args)))
        elif tag == 0x1e:
            rest = r.take(5)
            out.append((at, 'x1e', hx(rest)))
        elif tag == 0x1c:
            name = r.sz(); c = common(r, False); attrs_list(r, c)
            out.append((at, 'namespace', (name, c)))
        elif tag == 0x0b:
            z = r.u8(); i = r.id(); name = r.sz(); z2 = r.u8(); t = r.c32(); c = common(r, False); attrs_list(r, c)
            out.append((at, 'type', (z, hex(i), name, z2, hex(t), c)))
        elif tag == 0x08:
            z = r.u8(); i = r.id(); name = r.sz(); t = r.c32(); attrs = r.take(5); v = r.take(attrs[4])
            out.append((at, 'const', (z, hex(i), name, hex(t), hx(attrs), hx(v))))
        elif tag in (1, 2, 3):
            z = r.u8(); i = r.id(); fl = r.u8(); name = r.sz()
            mt = r.u8()
            al = r.u8() if mt & 0x40 else None
            attrs = r.take(4)
            size = r.c32()
            c = common(r, False)
            t = r.c32()
            align = r.u8() if tag == 2 else None
            refs = None
            if c['x'] is not None and c['x'] & 0x80:
                refs = [(hex(r.id()), r.u8()) for _ in range(2)]
            elif c['f0'] & 0x80:
                n = r.c32()
                refs = [(hex(r.id()), r.u8()) for _ in range(n)]
            attrs_list(r, c)
            out.append((at, 'data%d' % tag, (hex(i), fl, name, hex(mt), al, hx(attrs), size, c, hex(t), align, refs)))
        elif tag in (0x0e, 0x04, 0x10):
            i = r.id(); fl = r.u8(); name = r.sz()
            mt = r.u8()
            al = r.u8() if mt & 0x40 else None
            attrs = r.take(3)
            c = common(r, True)
            extra = b''
            t = r.c32(); tid = r.u32()
            if tag == 0x0e:
                il = r.c32(); sy = r.c32(); grow = r.c16(); eight = r.take(8)
                params = r.c16(); nref = r.c32()
                refs = [(hex(r.id()), r.u8()) for _ in range(nref)]
                attrs_list(r, c)
                out.append((at, 'fbody', (hex(i), fl, name, hex(mt), hx(attrs), c, hx(extra), hex(t), hex(tid), hex(il), hex(sy), grow, hx(eight), params, refs)))
            else:
                if tag == 0x10:
                    c['assoc'] = hex(r.id())
                attrs_list(r, c)
                out.append((at, 'fdecl%x' % tag, (hex(i), fl, name, hex(mt), hx(attrs), c, hx(extra), hex(t), hex(tid))))
        else:
            raise Stop('unknown tag %x at %x' % (tag, at))


if __name__ == '__main__':
    quiet = '-q' in sys.argv
    for path in [a for a in sys.argv[1:] if a != '-q']:
        g = sections(path)['.cil$gl']
        out = []
        try:
            walk(g, out)
            st = None
        except (Stop, IndexError, struct.error, ValueError) as e:
            st = e
        print('##', path, 'STOPPED: %s' % st if st else 'complete', len(out))
        show = out if not quiet else out[-6:]
        for at, k, v in show:
            if k in ('pre', 'file') and not quiet:
                continue
            print('%06x %s %s' % (at, k, v))
        if st:
            at = out[-1][0] if out else 0
            hexdump(g[at:at + 160], at)
