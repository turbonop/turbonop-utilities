"""Throwaway spike: turn <name>.obj's copy of shared_f (python weak.py <name>) into a NOLIBRARY weak
external whose alternate is shared_f_tl (defined in x.obj)."""
import struct, sys
d = bytearray(open(sys.argv[1] + ".obj", "rb").read())
symptr, nsyms = struct.unpack_from("<II", d, 8)
strptr = symptr + 18 * nsyms
strings = bytearray(d[strptr:])
# Find shared_f.
old = None
i = 0
while i < nsyms:
    at = symptr + 18 * i
    if d[at:at + 8] == b"shared_f" and d[at + 16] == 2:
        old = i
    i += 1 + d[at + 17]
assert old is not None
# The old definition becomes shared_z (a COMDAT nobody names).
d[symptr + 18 * old:symptr + 18 * old + 8] = b"shared_z"
# New symbols: the weak external and its alternate.
weak, alt = nsyms, nsyms + 2
name_off = len(strings)
strings += b"shared_f_tl\0"
struct.pack_into("<I", strings, 0, len(strings))
syms = b"shared_f" + struct.pack("<IhHBB", 0, 0, 0x20, 105, 1)
syms += struct.pack("<II", weak + 2, 1) + b"\0" * 10  # TagIndex, NOLIBRARY
syms += struct.pack("<II", 0, name_off) + struct.pack("<IhHBB", 0, 0, 0x20, 2, 0)
# Calls to the old symbol now name the weak external.
n = struct.unpack_from("<H", d, 2)[0]
for s in range(n):
    h = 20 + 40 * s
    rptr, nrel = struct.unpack_from("<I", d, h + 24)[0], struct.unpack_from("<H", d, h + 32)[0]
    for r in range(nrel):
        at = rptr + 10 * r + 4
        if struct.unpack_from("<I", d, at)[0] == old:
            struct.pack_into("<I", d, at, weak)
out = d[:strptr] + syms + strings
struct.pack_into("<I", out, 12, nsyms + 3)
open(sys.argv[1] + ".weak.obj", "wb").write(out)
print(sys.argv[1] + ".weak.obj: shared_f is weak (NOLIBRARY) -> shared_f_tl")
