"""Writes turbonop-zip's fixtures (testdata/zip) with Python's zipfile.

    python tools/zip/make.py

Every entry has a fixed date and time, so the archives are the same each
run (given the same zlib). What zipfile can't write (ZIP64 records on
small archives, data descriptors, flags, other disks) it's made to, or the
bytes are patched after: see each fixture.
"""

import io
import os
import struct
import zipfile

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "testdata", "zip")
WHEN = (2024, 2, 29, 13, 37, 58)
TEXT = b"".join(b"line %d of a text that deflates well\n" % i for i in range(200))


def info(name, method=zipfile.ZIP_STORED, comment=b"", extra=b""):
    z = zipfile.ZipInfo(name, WHEN)
    z.compress_type = method
    z.comment = comment
    z.extra = extra
    z.create_system = 3
    z.external_attr = 0o100644 << 16
    return z


def save(name, data):
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(data)


def archive(entries, comment=b"", stream=None, zip64=False):
    """An archive of (ZipInfo, data) pairs."""
    buf = stream or io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.comment = comment
        for zi, data in entries:
            if zi.is_dir():
                zi.external_attr = (0o40755 << 16) | 0x10
                z.writestr(zi, b"")
            else:
                with z.open(zi, "w", force_zip64=zip64) as f:
                    f.write(data)
    return buf


class Unseekable(io.RawIOBase):
    """A stream zipfile can't seek in: it writes data descriptors."""

    def __init__(self):
        self.data = bytearray()

    def writable(self):
        return True

    def write(self, b):
        self.data += b
        return len(b)

    def flush(self):
        pass


def patch_flags(data, bit):
    """`bit` set in every local and central header's flags."""
    d = bytearray(data)
    for sig, at in ((b"PK\x03\x04", 6), (b"PK\x01\x02", 8)):
        i = d.find(sig)
        while i >= 0:
            (f,) = struct.unpack_from("<H", d, i + at)
            struct.pack_into("<H", d, i + at, f | bit)
            i = d.find(sig, i + 4)
    return bytes(d)


def main():
    os.makedirs(OUT, exist_ok=True)

    # Stored: files, a directory, an empty file.
    save("stored.zip", archive([
        (info("hello.txt"), b"Hello, world!\n"),
        (info("dir/"), b""),
        (info("dir/empty"), b""),
        (info("dir/text.txt"), TEXT),
    ]).getvalue())

    # Deflated, with comments (the archive's and an entry's) and a mix of
    # methods.
    save("deflated.zip", archive([
        (info("text.txt", zipfile.ZIP_DEFLATED, comment=b"an entry's comment"), TEXT),
        (info("random.bin", zipfile.ZIP_DEFLATED), bytes((i * 7919 + 13) % 251 for i in range(3000))),
        (info("tiny", zipfile.ZIP_DEFLATED), b"x"),
        (info("stored.txt"), b"not compressed\n"),
    ], comment=b"the archive's comment").getvalue())

    # Data descriptors: written to a stream zipfile can't seek in.
    s = Unseekable()
    archive([
        (info("a.txt", zipfile.ZIP_DEFLATED), TEXT),
        (info("b.txt"), b"stored, with a descriptor\n"),
        (info("c.txt", zipfile.ZIP_DEFLATED), TEXT[:500]),
    ], stream=s)
    save("descriptor.zip", bytes(s.data))

    # ZIP64: every entry's sizes and offset in ZIP64 fields, and the ZIP64
    # end records, by lowering zipfile's limits.
    limits = (zipfile.ZIP64_LIMIT, zipfile.ZIP_FILECOUNT_LIMIT)
    zipfile.ZIP64_LIMIT, zipfile.ZIP_FILECOUNT_LIMIT = 0, 0
    try:
        save("zip64.zip", archive([
            (info("first.txt", zipfile.ZIP_DEFLATED), TEXT),
            (info("second.txt"), b"second\n"),
        ], zip64=True).getvalue())
        s = Unseekable()
        archive([
            (info("streamed.txt", zipfile.ZIP_DEFLATED), TEXT),
            (info("after.txt"), b"after\n"),
        ], stream=s, zip64=True)
        save("zip64-descriptor.zip", bytes(s.data))
    finally:
        zipfile.ZIP64_LIMIT, zipfile.ZIP_FILECOUNT_LIMIT = limits

    # Non-ASCII names (UTF-8, flag 11), and extra fields: an extended
    # timestamp and Info-ZIP's Unix owner.
    ut = struct.pack("<HHBi", 0x5455, 5, 1, 1709213878)
    ux = struct.pack("<HHBBIBI", 0x7875, 11, 1, 4, 1000, 4, 1000)
    save("unicode.zip", archive([
        (info("café/naïve.txt", extra=ut + ux), b"utf-8 names\n"),
        (info("日本語.txt", zipfile.ZIP_DEFLATED), TEXT[:100]),
        (info("plain.txt", extra=ut), b"ascii\n"),
    ]).getvalue())

    # The empty archive: an end record only.
    save("empty.zip", archive([]).getvalue())

    # Data before the archive, offsets from the archive's start (as `cat
    # sfx.exe a.zip` makes): readers find the shift.
    inner = archive([(info("inside.txt", zipfile.ZIP_DEFLATED), TEXT)]).getvalue()
    save("prefixed.zip", b"#!/bin/sh\necho a self-extractor's stub\nexit 0\n" + inner)

    # Data before the archive, offsets from the file's start (zipfile's
    # append mode on a file that isn't an archive).
    buf = io.BytesIO(b"MZ\x90\x00" + b"\x00" * 60)
    buf.seek(0, 2)
    with zipfile.ZipFile(buf, "a") as z:
        z.writestr(info("appended.txt", zipfile.ZIP_DEFLATED), TEXT)
    save("appended.zip", buf.getvalue())

    # Methods turbonop-zip names but doesn't decode.
    save("methods.zip", archive([
        (info("b.bz2", zipfile.ZIP_BZIP2), TEXT),
        (info("l.lzma", zipfile.ZIP_LZMA), TEXT),
        (info("d.txt", zipfile.ZIP_DEFLATED), TEXT),
    ]).getvalue())

    # Encrypted (flagged: the data isn't really).
    plain = archive([(info("secret.txt"), b"not really secret\n")]).getvalue()
    save("encrypted.zip", patch_flags(plain, 1))

    # A part of a split archive: the end record's disk is 1.
    d = bytearray(archive([(info("part.txt"), b"part\n")]).getvalue())
    eocd = d.rfind(b"PK\x05\x06")
    struct.pack_into("<HH", d, eocd + 4, 1, 1)
    save("split.zip", bytes(d))


if __name__ == "__main__":
    main()
