import struct, sys
src, dst = sys.argv[1], sys.argv[2]
d = bytearray(open(src, "rb").read())
shoff = struct.unpack_from("<Q", d, 0x28)[0]
size, count = struct.unpack_from("<HH", d, 0x3A)
n = 0
for i in range(count):
    h = shoff + i * size
    kind = struct.unpack_from("<I", d, h + 4)[0]
    flags = struct.unpack_from("<Q", d, h + 8)[0]
    if flags & 2 and not flags & 4 and kind in (1, 14, 15, 16):
        struct.pack_into("<Q", d, h + 0x20, 1 << 40)
        n += 1
open(dst, "wb").write(d)
print("damaged", n)
