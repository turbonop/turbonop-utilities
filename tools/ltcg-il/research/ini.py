"""Experimental walker of .cil$in (initializers)."""
import struct, sys
from il import sections, hexdump
from walk import R, Stop, hx


def walk(d, out):
    r = R(d, 0)
    cur = None
    while True:
        at = r.p
        tag = r.u8()
        if tag == 0x0f:
            if r.p != len(d):
                raise Stop('0f at %x of %x' % (at, len(d)))
            out.append((at, 'END', None))
            return
        if tag == 0x00:
            i = r.id(); z = r.c32()
            out.append((at, 'begin', (hex(i), z)))
        elif tag == 0x07:
            out.append((at, 'end', None))
        elif tag == 0x01:
            kind = r.u8(); size = r.c32()
            if kind == 5:
                v = r.take(size)
            elif size == 1:
                v = r.take(1)
            else:
                b = r.u8()
                v = r.take(size) if b == 0x80 else bytes([b])
            out.append((at, 'scalar', (kind, size, hx(v))))
        elif tag in (0x02, 0x12):
            i = r.id(); add = r.c32(); size = r.c32()
            out.append((at, 'addr' if tag == 2 else 'rva', (hex(i), add, size)))
        elif tag == 0x03:
            n = r.c16(); v = r.take(n)
            out.append((at, 'bytes', v))
        elif tag == 0x08:
            n = r.c32()
            out.append((at, 'zeros', n))
        else:
            raise Stop('in tag %x at %x' % (tag, at))


if __name__ == '__main__':
    quiet = '-q' in sys.argv
    for path in [a for a in sys.argv[1:] if a != '-q']:
        d = sections(path).get('.cil$in', b'')
        out = []
        try:
            walk(d, out)
            st = None
        except (Stop, IndexError, struct.error, ValueError) as e:
            st = e
        print('##', path, 'STOPPED: %s' % st if st else 'complete', len(out))
        show = out if not quiet else out[-8:]
        for at, k, v in show:
            print('%06x %s %s' % (at, k, v))
        if st:
            at = out[-1][0] if out else 0
            hexdump(d[at:at + 128], at)
