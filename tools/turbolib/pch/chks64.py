"""a.stale.obj: a.obj with its stack size changed (sub/add rsp, imm8, plus 8) and
.chks64 left as it was. a.zeroed.obj: a.obj with .chks64's contents zeroed."""
import struct
d = bytearray(open("a.obj", "rb").read())
n = struct.unpack_from("<H", d, 2)[0]
secs = [(bytes(d[20 + 40 * i:28 + 40 * i]).rstrip(b"\0"), 20 + 40 * i) for i in range(n)]
done = False
for name, h in secs:
    if not name.startswith(b".text"):
        continue
    size, ptr = struct.unpack_from("<II", d, h + 16)
    code = d[ptr:ptr + size]
    k = code.find(b"\x48\x83\xec")
    if k < 0:
        continue
    bad = bytearray(d)
    bad[ptr + k + 3] = (bad[ptr + k + 3] + 8) & 0xFF
    j = code.find(b"\x48\x83\xc4", k)
    if j >= 0:
        bad[ptr + j + 3] = (bad[ptr + j + 3] + 8) & 0xFF
    open("a.stale.obj", "wb").write(bad)
    print("a.stale.obj: changed", name, "at", hex(k), "without updating .chks64")
    done = True
    break
assert done
for name, h in secs:
    if name == b".chks64":
        size, ptr = struct.unpack_from("<II", d, h + 16)
        z = bytearray(d)
        z[ptr:ptr + size] = b"\0" * size
        open("a.zeroed.obj", "wb").write(z)
        print("a.zeroed.obj: .chks64 zeroed")
