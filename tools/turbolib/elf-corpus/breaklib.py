#!/usr/bin/env python3
"""Changes one byte of one function in a copy of a static library, then
links a program against it, so that `turbolib verify` and
`turbolib compare-images` can be shown to catch a change they must.

    breaklib.py --build ~/tlcorpus/build-gcc --lib DIR/libLLVMSupport.a
                --member raw_ostream.cpp.o
                --section .text._ZN4llvm11raw_ostream5writeEPKcm
                --out DIR [--linker lld] [--program llvm-nm]
                [--folded DIR]

The byte is the section's last (a function's `ret`, most often), XORed
with 0x01; the member keeps its size, so the archive and its index stay
valid. The broken copy is written to DIR/<library>, and the program,
linked with the build's link command against the libraries of --folded
(if given, else the build's) with the broken library in its place, to
DIR/<program>-<linker>-broken.
"""

import argparse
import os
import shutil
import struct
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import relink  # noqa: E402


def members(data):
    """(name, data offset, size) of each member of a GNU archive."""
    assert data.startswith(b"!<arch>\n")
    at = 8
    longnames = b""
    out = []
    while at + 60 <= len(data):
        h = data[at:at + 60]
        name = h[:16].rstrip()
        size = int(h[48:58])
        body = at + 60
        if name == b"//":
            longnames = data[body:body + size]
        elif name.startswith(b"/") and name[1:].isdigit():
            off = int(name[1:])
            end = longnames.index(b"/\n", off)
            out.append((longnames[off:end], body, size))
        elif name not in (b"/", b"/SYM64/"):
            out.append((name.rstrip(b"/"), body, size))
        at = body + size + (size & 1)
    return out


def section(obj, wanted):
    """The file offset and size of the section `wanted` of an ELF64
    little-endian object (extended numbering included)."""
    shoff, = struct.unpack_from("<Q", obj, 0x28)
    shnum, shstrndx = struct.unpack_from("<HH", obj, 0x3C)
    if shnum == 0:
        shnum, = struct.unpack_from("<Q", obj, shoff + 0x20)
    if shstrndx == 0xFFFF:
        shstrndx, = struct.unpack_from("<I", obj, shoff + 0x2C)

    def header(i):
        return struct.unpack_from("<IIQQQQIIQQ", obj, shoff + 64 * i)

    names = header(shstrndx)
    strs = obj[names[4]:names[4] + names[5]]
    for i in range(shnum):
        h = header(i)
        n = strs[h[0]:strs.index(b"\0", h[0])]
        if n == wanted:
            return h[4], h[5]
    raise SystemExit(f"no section {wanted!r}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", required=True)
    ap.add_argument("--lib", required=True)
    ap.add_argument("--member", required=True)
    ap.add_argument("--section", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--linker", default="lld")
    ap.add_argument("--program", default="llvm-nm")
    ap.add_argument("--folded")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    data = bytearray(open(args.lib, "rb").read())
    for name, body, size in members(bytes(data)):
        if name == args.member.encode():
            off, n = section(bytes(data[body:body + size]), args.section.encode())
            at = body + off + n - 1
            data[at] ^= 0x01
            print(f"{args.member} {args.section}: byte {n - 1} now {data[at]:#04x}")
            break
    else:
        raise SystemExit(f"no member {args.member}")
    # The libraries to link: the folded ones (or the build's), this one
    # broken.
    libs = os.path.join(args.out, "libs")
    os.makedirs(libs, exist_ok=True)
    if args.folded:
        for f in os.listdir(args.folded):
            dst = os.path.join(libs, f)
            if not os.path.exists(dst):
                os.symlink(os.path.join(os.path.abspath(args.folded), f), dst)
    dst = os.path.join(libs, os.path.basename(args.lib))
    if os.path.lexists(dst):
        os.remove(dst)
    open(dst, "wb").write(data)
    shutil.copy(dst, os.path.join(args.out, os.path.basename(args.lib)))
    build = os.path.abspath(args.build)
    exe = os.path.join(os.path.abspath(args.out), f"{args.program}-{args.linker}-broken")
    argv = relink.for_linker(relink.link_command(build, args.program), args.linker, exe, libs, False)
    p = subprocess.run(argv, cwd=build, capture_output=True, text=True)
    if p.returncode != 0:
        raise SystemExit(f"link failed: {p.stderr[-2000:]}")
    print(exe)


if __name__ == "__main__":
    main()
