"""Copies a static library with one byte of one function's code changed,
for checking that relink.py's comparison catches a broken library.

    python breaklib.py <library> <out library> <symbol> [--offset N]

<symbol> is the decorated name of an external symbol a member defines in
a section (a function). The byte at --offset (default 4) from the symbol's
start is inverted; nothing else changes, so
the archive's index and every offset stay valid. Members may be regular or
big-obj COFF objects.
"""
import argparse
import struct
import sys

BIGOBJ_CLASS_ID = bytes([0xC7, 0xA1, 0xBA, 0xD1, 0xEE, 0xBA, 0xA9, 0x4B,
                         0xAF, 0x20, 0xFA, 0xF6, 0x6A, 0xA4, 0xDC, 0xB8])


def members(data):
    """(name field, data offset, size) of each archive member."""
    at = 8
    while at + 60 <= len(data):
        header = data[at:at + 60]
        size = int(header[48:58].decode().strip() or 0)
        yield header[:16].decode(errors="replace").strip(), at + 60, size
        at += 60 + size + (size & 1)


def find(data, start, size, symbol):
    """The file offset of `symbol`'s first byte in the object at `start`,
    or None."""
    obj = data[start:start + size]
    if len(obj) < 20:
        return None
    if obj[:4] == b"\0\0\xff\xff":
        if len(obj) < 56 or obj[12:28] != BIGOBJ_CLASS_ID:
            return None
        sections, table, count = struct.unpack_from("<III", obj, 44)
        header, record = 56, 20
    else:
        sections = struct.unpack_from("<H", obj, 2)[0]
        table, count = struct.unpack_from("<II", obj, 8)
        header = 20 + struct.unpack_from("<H", obj, 16)[0]
        record = 18
    strings = table + count * record
    want = symbol.encode()
    i = 0
    while i < count:
        at = table + i * record
        raw = obj[at:at + 8]
        if raw[:4] == b"\0\0\0\0":
            off = struct.unpack_from("<I", raw, 4)[0]
            end = obj.index(b"\0", strings + off)
            name = obj[strings + off:end]
        else:
            name = raw.rstrip(b"\0")
        if record == 20:
            section = struct.unpack_from("<i", obj, at + 12)[0]
            storage, aux = obj[at + 18], obj[at + 19]
        else:
            section = struct.unpack_from("<h", obj, at + 12)[0]
            storage, aux = obj[at + 16], obj[at + 17]
        if name == want and storage == 2 and 0 < section <= sections:
            sh = header + (section - 1) * 40
            pointer = struct.unpack_from("<I", obj, sh + 20)[0]
            value = struct.unpack_from("<I", obj, at + 8)[0]
            return start + pointer + value
        i += 1 + aux
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("library")
    ap.add_argument("out")
    ap.add_argument("symbol")
    ap.add_argument("--offset", type=int, default=4)
    a = ap.parse_args()
    data = bytearray(open(a.library, "rb").read())
    for name, start, size in members(data):
        if name.startswith("/") and not name[1:].isdigit():
            continue
        at = find(data, start, size, a.symbol)
        if at is not None:
            data[at + a.offset] ^= 0xFF
            open(a.out, "wb").write(data)
            print(f"{a.symbol}: changed the byte at file offset {at + a.offset:#x} (member {name})")
            return 0
    print(f"{a.symbol}: no member defines it in a section", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
