"""Throwaway: COFF objects each with one ~1.1 GiB .rdata section, defining big_<n>."""
import struct
SIZE = 1100 * 1024 * 1024
for n in range(1, 6):
    name = b"big_%d" % n
    header = struct.pack("<HHIIIHH", 0x8664, 1, 0, 0, 0, 0, 0)
    data_at = 20 + 40
    sym_at = data_at + SIZE
    sec = struct.pack("<8sIIIIIIHHI", b".rdata", 0, 0, SIZE, data_at, 0, 0, 0, 0, 0x40400040)
    syms = b".rdata\0\0" + struct.pack("<IhHBB", 0, 1, 0, 3, 1) + struct.pack("<IHHIHBB2s", SIZE, 0, 0, 0, 0, 0, 0, b"\0\0")
    syms += name.ljust(8, b"\0") + struct.pack("<IhHBB", 0, 1, 0, 2, 0)
    header = struct.pack("<HHIIIHH", 0x8664, 1, 0, sym_at, 3, 0, 0)
    with open("big%d.obj" % n, "wb") as f:
        f.write(header + sec)
        chunk = b"\x5A" * (64 * 1024 * 1024)
        left = SIZE
        while left:
            k = min(left, len(chunk)); f.write(chunk[:k]); left -= k
        f.write(syms + struct.pack("<I", 4))
print("made big1..5.obj")
