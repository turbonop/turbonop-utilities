"""Throwaway spike: a synthesized precompiled-types object (.debug$P) that no
compiler made, and users rewritten to name it with LF_PRECOMP.

  python spike.py full  a.obj               -> a.pch.obj, a.user.obj
  python spike.py share a.obj b.obj         -> ab.pch.obj, a.user.obj, b.user.obj

`full` moves every type record of one object into the PCH; `share` moves the
longest common prefix of two objects' records (byte-identical, so their
indices agree) and leaves each its own remainder, indices unchanged.
"""
import struct
import sys

LF_PRECOMP = 0x1509
LF_ENDPRECOMP = 0x0014
S_OBJNAME = 0x1101
SIGNATURE = 0x7B1C0DE5
PCH_SYMBOL = b"__@@_PchSym_turbolib_spike"


def sections(d):
    n = struct.unpack_from("<H", d, 2)[0]
    opt = struct.unpack_from("<H", d, 16)[0]
    out = []
    for i in range(n):
        at = 20 + opt + 40 * i
        h = d[at:at + 40]
        size, ptr = struct.unpack_from("<II", h, 16)
        out.append({"index": i + 1, "header_at": at, "name": h[:8].rstrip(b"\0"), "size": size, "ptr": ptr})
    return out


def records(sec):
    assert struct.unpack_from("<I", sec, 0)[0] == 4
    at, out = 4, []
    while at < len(sec):
        ln = struct.unpack_from("<H", sec, at)[0]
        out.append(sec[at:at + 2 + ln])
        at += 2 + ln
    return out


def debug_t(d):
    s = [s for s in sections(d) if s["name"] == b".debug$T"][0]
    return s, records(d[s["ptr"]:s["ptr"] + s["size"]])


def record(kind, body):
    # The whole record (length field included) is a multiple of 4, padded
    # with LF_PAD bytes counting down (F3 F2 F1).
    pad = (-len(body)) % 4
    body = body + bytes(0xF0 + pad - i for i in range(pad))
    return struct.pack("<HH", len(body) + 2, kind) + body


def precomp_record(count, name):
    return record(LF_PRECOMP, struct.pack("<III", 0x1000, count, SIGNATURE) + name + b"\0")


def symbol_aux(d, section_index):
    """The offset of the section-definition aux record of section `section_index`."""
    ptr, count = struct.unpack_from("<II", d, 8)
    i = 0
    while i < count:
        at = ptr + 18 * i
        sec = struct.unpack_from("<h", d, at + 12)[0]
        cls, naux = d[at + 16], d[at + 17]
        value = struct.unpack_from("<I", d, at + 8)[0]
        if sec == section_index and cls == 3 and value == 0 and naux:
            return at + 18
        i += 1 + naux
    return None


def replace_section(d, sec, data):
    """Appends `data` and points `sec` at it (its aux length too)."""
    d = bytearray(d)
    while len(d) % 4:
        d.append(0)
    at = len(d)
    d += data
    struct.pack_into("<II", d, sec["header_at"] + 16, len(data), at)
    aux = symbol_aux(d, sec["index"])
    if aux is not None:
        struct.pack_into("<I", d, aux, len(data))
    return bytes(d)


def add_directive(d, text):
    """Appends `text` to .drectve (moving it to the file's end)."""
    s = [s for s in sections(d) if s["name"] == b".drectve"][0]
    old = d[s["ptr"]:s["ptr"] + s["size"]]
    return replace_section(d, s, old.rstrip(b"\0") + b" " + text + b" ")


def feat00(d):
    ptr, count = struct.unpack_from("<II", d, 8)
    i = 0
    while i < count:
        at = ptr + 18 * i
        if d[at:at + 8] == b"@feat.00":
            return struct.unpack_from("<I", d, at + 8)[0]
        i += 1 + d[at + 17]
    return 0


def pch_object(path, type_records, feat):
    """A COFF object of: .debug$S (S_OBJNAME), .bss (the PCH symbol), .debug$P."""
    name = path.encode()
    objname = struct.pack("<HI", S_OBJNAME, 0) + name + b"\0"
    objname = struct.pack("<H", len(objname)) + objname
    sub = objname + b"\0" * ((-len(objname)) % 4)
    debug_s = struct.pack("<III", 4, 0xF1, len(objname)) + sub
    debug_p = struct.pack("<I", 4) + b"".join(type_records) + record(LF_ENDPRECOMP, struct.pack("<I", SIGNATURE))
    secs = [
        (b".debug$S", debug_s, 0x42100040),  # CNT_INITIALIZED | DISCARDABLE | READ | ALIGN_1
        (b".bss", b"", 0xC0300080),  # UNINITIALIZED | READ | WRITE | ALIGN_4
        (b".debug$P", debug_p, 0x42100040),
    ]
    header = 20
    at = header + 40 * len(secs)
    raw = b""
    headers = b""
    for nm, data, ch in secs:
        if nm == b".bss":
            headers += struct.pack("<8sIIIIIIHHI", nm, 0, 0, 4, 0, 0, 0, 0, 0, ch)
            continue
        while (at + len(raw)) % 4:
            raw += b"\0"
        headers += struct.pack("<8sIIIIIIHHI", nm, 0, 0, len(data), at + len(raw), 0, 0, 0, 0, ch)
        raw += data
    strings = b""
    symbols = b""
    nsyms = 0

    def sym(nm, value, section, cls, aux=b""):
        nonlocal strings, symbols, nsyms
        if len(nm) > 8:
            field = struct.pack("<II", 0, 4 + len(strings))
            strings += nm + b"\0"
        else:
            field = nm.ljust(8, b"\0")
        symbols += field + struct.pack("<IhHBB", value, section, 0, cls, len(aux) // 18) + aux
        nsyms += 1 + len(aux) // 18

    sym(b"@feat.00", feat, -1, 3)
    for i, (nm, data, ch) in enumerate(secs):
        length = 4 if nm == b".bss" else len(data)
        sym(nm, 0, i + 1, 3, struct.pack("<IHHIHBB2s", length, 0, 0, 0, 0, 0, 0, b"\0\0") + b"\0" * 0)
    sym(PCH_SYMBOL, 0, 2, 2)
    symptr = at + len(raw)
    body = raw + symbols + struct.pack("<I", 4 + len(strings)) + strings
    head = struct.pack("<HHIIIHH", 0x8664, len(secs), 0, symptr, nsyms, 0, 0)
    return head + headers + body


def fix_aux_sizes(obj):
    return obj


def main():
    mode = sys.argv[1]
    if mode == "full":
        a = open(sys.argv[2], "rb").read()
        sec, recs = debug_t(a)
        stem = sys.argv[2][:-4]
        open(stem + ".pch.obj", "wb").write(pch_object(stem + ".pch.obj", recs, feat00(a)))
        user = replace_section(a, sec, struct.pack("<I", 4) + precomp_record(len(recs), (stem + ".pch.obj").encode()))
        user = add_directive(user, b"-INCLUDE:" + PCH_SYMBOL)
        open(stem + ".user.obj", "wb").write(user)
        print(f"{len(recs)} records moved into {stem}.pch.obj")
    elif mode == "share":
        objs = [open(p, "rb").read() for p in sys.argv[2:4]]
        parsed = [debug_t(o) for o in objs]
        p = 0
        while all(p < len(r) for _, r in parsed) and parsed[0][1][p] == parsed[1][1][p]:
            p += 1
        open("ab.pch.obj", "wb").write(pch_object("ab.pch.obj", parsed[0][1][:p], feat00(objs[0])))
        for path, o, (sec, recs) in zip(sys.argv[2:4], objs, parsed):
            body = struct.pack("<I", 4) + precomp_record(p, b"ab.pch.obj") + b"".join(recs[p:])
            user = add_directive(replace_section(o, sec, body), b"-INCLUDE:" + PCH_SYMBOL)
            open(path[:-4] + ".share.obj", "wb").write(user)
            print(f"{path}: {p} shared records, {len(recs) - p} its own")




def prefix(path, count, tag):
    a = open(path, "rb").read()
    sec, recs = debug_t(a)
    stem = path[:-4] + "." + tag
    open(stem + ".pch.obj", "wb").write(pch_object(stem + ".pch.obj", recs[:count], feat00(a)))
    body = struct.pack("<I", 4) + precomp_record(count, (stem + ".pch.obj").encode()) + b"".join(recs[count:])
    user = add_directive(replace_section(a, sec, body), b"-INCLUDE:" + PCH_SYMBOL)
    open(stem + ".user.obj", "wb").write(user)
    print(f"{stem}: {count} records in the PCH, {len(recs) - count} its own")


if sys.argv[1] == "prefix":
    prefix(sys.argv[2], int(sys.argv[3]), sys.argv[4])
else:
    main()
