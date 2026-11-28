"""Breaks a folded library the way a fault in folding could: in the member
that defines a COMDAT, changes its bytes or sends one of its relocations to
another symbol, in place (the archive's sizes and indexes unchanged).

    python breakfold.py <in.lib> <out.lib> bytes <symbol> <old hex> <new hex>
    python breakfold.py <in.lib> <out.lib> retarget <symbol> <from> <to>

`bytes` replaces the only occurrence of <old hex> in <symbol>'s section with
<new hex> (as long). `retarget` points the relocation in <symbol>'s section
that names <from> at <to> instead (both symbols of the same member).
"""
import struct
import sys


def members(data):
    """(offset of the member's data, its size, its name) of each member."""
    assert data[:8] == b"!<arch>\n"
    at = 8
    while at + 60 <= len(data):
        h = data[at:at + 60]
        name = h[:16].rstrip(b" ")
        size = int(h[48:58].strip())
        yield at + 60, size, name
        at += 60 + size + (size & 1)


def symbols(obj):
    """The COFF object's sections (name, raw offset, raw size, relocations'
    offset, count) and symbols (index, name, value, section)."""
    machine, nsec, _, symptr, nsym, opt, _ = struct.unpack_from("<HHIIIHH", obj, 0)
    strings = symptr + 18 * nsym

    def name_of(raw):
        if raw[:4] == b"\0\0\0\0":
            o = struct.unpack_from("<I", raw, 4)[0]
            end = obj.index(b"\0", strings + o)
            return obj[strings + o:end]
        return raw.rstrip(b"\0")

    sections = []
    for i in range(nsec):
        o = 20 + opt + 40 * i
        raw_size, raw_ptr, reloc_ptr = struct.unpack_from("<III", obj, o + 16)
        nrel = struct.unpack_from("<H", obj, o + 32)[0]
        sections.append((obj[o:o + 8], raw_ptr, raw_size, reloc_ptr, nrel))
    syms = []
    i = 0
    while i < nsym:
        o = symptr + 18 * i
        value, section = struct.unpack_from("<Ih", obj, o + 8)
        aux = obj[o + 17]
        syms.append((i, name_of(obj[o:o + 8]), value, section))
        i += 1 + aux
    return sections, syms


def main():
    src, dst, mode, symbol = sys.argv[1:5]
    data = bytearray(open(src, "rb").read())
    symbol = symbol.encode()
    done = False
    for start, size, name in members(data):
        if name in (b"/", b"//"):
            continue
        obj = bytes(data[start:start + size])
        if obj[:4] in (b"\0\0\xff\xff",) or len(obj) < 20:
            continue
        sections, syms = symbols(obj)
        defs = [s for s in syms if s[1] == symbol and s[3] > 0]
        if not defs:
            continue
        sec = sections[defs[0][3] - 1]
        _, raw_ptr, raw_size, reloc_ptr, nrel = sec
        if mode == "bytes":
            old, new = bytes.fromhex(sys.argv[5]), bytes.fromhex(sys.argv[6])
            assert len(old) == len(new)
            body = obj[raw_ptr:raw_ptr + raw_size]
            assert body.count(old) == 1, f"{body.count(old)} occurrences in {body.hex()}"
            at = start + raw_ptr + body.index(old)
            data[at:at + len(old)] = new
        else:
            by_name = {s[1]: s[0] for s in syms}
            frm, to = by_name[sys.argv[5].encode()], by_name[sys.argv[6].encode()]
            hits = [k for k in range(nrel)
                    if struct.unpack_from("<I", obj, reloc_ptr + 10 * k + 4)[0] == frm]
            assert len(hits) >= 1, "no relocation names it"
            at = start + reloc_ptr + 10 * hits[0] + 4
            data[at:at + 4] = struct.pack("<I", to)
        done = True
        print(f"{mode} in member {name.decode(errors='replace')}")
        break
    assert done, "no member defines it"
    open(dst, "wb").write(data)


if __name__ == "__main__":
    main()
