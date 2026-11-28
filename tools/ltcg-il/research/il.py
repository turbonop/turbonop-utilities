"""Experimental LTCG object dumper: sections, and hex of chosen ones."""
import struct, sys


def sections(path):
    d = open(path, 'rb').read()
    assert d[0:4] == b'\0\0\xff\xff'
    il = d[32:32 + struct.unpack_from('<I', d, 28)[0]]
    n = struct.unpack_from('<H', il, 2)[0]
    symtab, nsyms = struct.unpack_from('<II', il, 8)
    opt = struct.unpack_from('<H', il, 16)[0]
    strings = symtab + nsyms * 18
    out = {}
    for i in range(n):
        h = il[20 + opt + i * 40:][:40]
        name = h[:8]
        if name[0:1] == b'/':
            off = int(name[1:].rstrip(b'\0'))
            s = il[strings + off:]
            name = s[:s.index(0)]
        else:
            name = name.rstrip(b'\0')
        size, ptr = struct.unpack_from('<II', h, 16)
        out[name.decode()] = il[ptr:ptr + size]
    return out


def hexdump(b, base=0, width=16):
    for i in range(0, len(b), width):
        c = b[i:i + width]
        print('%06x  %-48s %s' % (base + i, ' '.join('%02x' % x for x in c),
                                   ''.join(chr(x) if 32 <= x < 127 else '.' for x in c)))


if __name__ == '__main__':
    s = sections(sys.argv[1])
    want = sys.argv[2:] or None
    for k, v in s.items():
        print('== %s %d (0x%x)' % (k, len(v), len(v)))
        if want and k in want:
            hexdump(v)
