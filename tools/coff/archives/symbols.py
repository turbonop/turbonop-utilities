"""Writes COFF objects holding every kind of symbol, for measuring which
ones lib.exe and llvm-lib index, and how they choose between members that
define the same name. Run from the directory to write them into (see
..\\archives.cmd).

Each object has a .text section and a COMDAT .text$mn section, then the
symbols listed for it: (name, value, section, type, storage class,
auxiliary records)."""
import os
import struct

EXTERNAL, STATIC, WEAK_EXTERNAL = 2, 3, 105


def obj(path, symbols, machine=0x8664):
    strings = bytearray(b"\0\0\0\0")
    records = bytearray()
    count = 0
    for name, value, section, kind, storage, aux in symbols:
        raw = name.encode("latin1")
        if len(raw) <= 8:
            field = raw.ljust(8, b"\0")
        else:
            field = struct.pack("<II", 0, len(strings))
            strings += raw + b"\0"
        records += field + struct.pack("<IhHBB", value, section, kind, storage, len(aux))
        for a in aux:
            records += a.ljust(18, b"\0")
        count += 1 + len(aux)
    struct.pack_into("<I", strings, 0, len(strings))
    sections = 2
    data_at = 20 + 40 * sections
    headers = bytearray()
    for i in range(sections):
        name = b".text" if i == 0 else b".text$mn"
        flags = 0x60000020 | (0x1000 if i else 0)
        headers += name.ljust(8, b"\0") + struct.pack(
            "<IIIIIIHHI", 0, 0, 4, data_at + 4 * i, 0, 0, 0, 0, flags)
    body = b"\xc3" * (4 * sections)
    header = struct.pack("<HHIIIHH", machine, sections, 0, data_at + len(body), count, 0, 0)
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "wb") as f:
        f.write(header + headers + body + records + strings)


def secdef(selection=0):
    return struct.pack("<IHHIHB", 4, 0, 0, 0, 0, selection)


def weak(tag, kind):
    return struct.pack("<II", tag, kind)


SECTIONS = [(".text", 0, 1, 0, STATIC, [secdef()]), (".text$mn", 0, 2, 0, STATIC, [secdef(2)])]


def defining(path, names):
    obj(path, SECTIONS + [(n, 0, 1, 0, EXTERNAL, []) for n in names])


# Every kind of symbol (index 4 is the first after the sections).
obj("kinds.obj", SECTIONS + [
    ("comdat_leader", 0, 2, 0x20, EXTERNAL, []),        # 4
    ("ext_def", 0, 1, 0x20, EXTERNAL, []),
    ("ext_undef", 0, 0, 0, EXTERNAL, []),
    ("ext_common", 8, 0, 0, EXTERNAL, []),
    ("ext_abs", 5, -1, 0, EXTERNAL, []),
    ("static_def", 0, 1, 0, STATIC, []),
    ("weak_nolib", 0, 0, 0, WEAK_EXTERNAL, [weak(4, 1)]),
    ("weak_lib", 0, 0, 0, WEAK_EXTERNAL, [weak(4, 2)]),
    ("weak_alias", 0, 0, 0, WEAK_EXTERNAL, [weak(4, 3)]),
    ("weak_anti", 0, 0, 0, WEAK_EXTERNAL, [weak(4, 4)]),
    ("lbl", 0, 1, 0, 6, []),
    ("extdef5", 0, 1, 0, 5, []),
    ("clrtok", 0, -1, 0, 107, []),
    (".bf", 0, 1, 0, 101, [b""]),
    ("undef_static", 0, 0, 0, 14, []),
    ("ext_long_name_more_than_8", 0, 1, 0, EXTERNAL, []),
    ("weak_in_sect", 0, 1, 0, WEAK_EXTERNAL, [weak(4, 3)]),
    ("abcdefgh", 0, 1, 0, EXTERNAL, []),
    ("ext_val2", 2, 1, 0, EXTERNAL, []),
    ("undef_label", 0, 0, 0, 7, []),
    ("sect_class", 0, 1, 0, 104, []),
])
# Weak externals defined, absolute, or with a value.
obj("weak.obj", SECTIONS + [
    ("target", 0, 1, 0x20, EXTERNAL, []),               # 4
    ("w_s1_c1", 0, 1, 0, WEAK_EXTERNAL, [weak(4, 1)]),
    ("w_s1_c2", 0, 1, 0, WEAK_EXTERNAL, [weak(4, 2)]),
    ("w_s1_c4", 0, 1, 0, WEAK_EXTERNAL, [weak(4, 4)]),
    ("w_s0v5_c1", 5, 0, 0, WEAK_EXTERNAL, [weak(4, 1)]),
    ("w_s0v5_c3", 5, 0, 0, WEAK_EXTERNAL, [weak(4, 3)]),
    ("w_abs_c2", 0, -1, 0, WEAK_EXTERNAL, [weak(4, 2)]),
    ("e_abs0", 0, -1, 0, EXTERNAL, []),
    ("fn_undef", 0, 0, 0x20, EXTERNAL, []),
    ("static_abs", 0, -1, 0, STATIC, []),
    ("e_aux", 0, 1, 0x20, EXTERNAL, [b"\x01\x02"]),
])
# The same names defined, common, and weak in several members.
obj("c_common.obj", SECTIONS + [
    ("x", 4, 0, 0, EXTERNAL, []), ("y", 4, 0, 0, EXTERNAL, []), ("z", 0, 1, 0, EXTERNAL, []),
    ("w", 8, 0, 0, EXTERNAL, []), ("wk", 0, 1, 0, WEAK_EXTERNAL, [weak(4, 3)])])
obj("c_def.obj", SECTIONS + [
    ("x", 0, 1, 0, EXTERNAL, []), ("z", 4, 0, 0, EXTERNAL, []), ("w", 16, 0, 0, EXTERNAL, []),
    ("wk", 0, 1, 0, EXTERNAL, []), ("abs1", 0, -1, 0, EXTERNAL, [])])
obj("c_weak.obj", SECTIONS + [
    ("wk2", 0, 1, 0, WEAK_EXTERNAL, [weak(4, 3)]), ("abs1", 4, 0, 0, EXTERNAL, [])])
defining("c_def2.obj", ["wk2"])
obj("d1.obj", SECTIONS + [("big_first", 16, 0, 0, EXTERNAL, []), ("eq", 8, 0, 0, EXTERNAL, [])])
obj("d2.obj", SECTIONS + [("big_first", 8, 0, 0, EXTERNAL, []), ("eq", 8, 0, 0, EXTERNAL, [])])
obj("d3.obj", SECTIONS + [("big_first", 4, 0, 0, EXTERNAL, []), ("eq", 8, 0, 0, EXTERNAL, [])])
# Names that sort by their bytes, and duplicates across members.
defining("s1.obj", ["zeta", "alpha", "Beta", "_under", "aa", "a", "\x80hi", "~t", "A", "a_b",
                    "a.b", "a@b", "?x@@YAXXZ", "??_C@_0x", "a$b", "alpha", "dupself"])
defining("s2.obj", ["mid", "alpha", "b", "Zeta", "__imp_q", "zeta2"])
defining("s3_with_a_long_name.obj", ["c3", "alpha", "A"])
defining("short.obj", ["aaa", "zzz"])
# Member names of 13 to 16 characters, and paths.
defining("n23456789.obj", ["n13"])
defining("n234567890.obj", ["n14"])
defining("n234567890a.obj", ["n15"])
defining("n234567890ab.obj", ["n16"])
defining("n234567890abc.obj", ["n17"])
defining(os.path.join("sub", "x.obj"), ["subx"])
defining(os.path.join("sub2", "x.obj"), ["sub2x"])
defining(os.path.join("sub", "long_name_again.obj"), ["l1"])
defining(os.path.join("sub2", "long_name_again.obj"), ["l2"])
defining(os.path.join("sub", "fs.obj"), ["fs"])
defining("noext.obj", [])
