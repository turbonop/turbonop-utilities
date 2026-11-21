"""Writes crates/isa/powerpc/tests/golden.txt: LLVM's text (llvm-objdump
-d) for encodings of each PowerPC instruction hexc's decoder knows (up to
four of each, their operand bits random), and for random words and
prefixed pairs, on 64-bit little- and big-endian and 32-bit big-endian
PowerPC; `<unknown>` where LLVM decodes nothing.

Usage: python oracle.py <LLVM bin directory> <crates/isa/powerpc>

The encodings come from the crate's tables.rs (each instruction's fixed
bits); the words are assembled with `.long` (llvm-mc: a prefixed
instruction's prefix, then its suffix) and disassembled at their
addresses. Seeded: the same tables give the same file.
"""

import os
import random
import re
import subprocess
import sys
import tempfile

PER_INSTRUCTION = 4
RANDOM_WORDS = 3000
RANDOM_PREFIXED = 500

# Instructions random words rarely are: those LLVM's printer writes by
# hand (the shifts, the cache hints), aliases, and operands at their edges.
EDGES = """
    beq 0, .+8
    bne 7, .-4
    bdnz .+8
    blr
    beqlr 1
    bctrl
    bl .+16
    b .-8
    ba 0x100
    bla 0x2000
    lwz 3, 8(1)
    lwz 3, -8(0)
    ld 4, -16(1)
    stdu 1, -32(1)
    lwzx 3, 4, 5
    lwzx 3, 0, 5
    li 3, -1
    lis 4, 0x1234
    addi 3, 3, 1
    addis 3, 2, 1
    mflr 0
    mtlr 0
    mtctr 12
    slwi 3, 4, 2
    slwi 3, 4, 0
    srwi 3, 4, 2
    srwi 3, 4, 31
    sldi 3, 4, 2
    sldi 3, 4, 63
    rlwinm 3, 4, 2, 3, 4
    rldicr 3, 4, 8, 50
    clrldi 3, 4, 32
    xxlor 34, 35, 35
    xxlor 1, 2, 3
    vaddubm 2, 3, 4
    fadd 1, 2, 3
    cmpwi 3, 0
    cmpd 7, 3, 4
    isel 3, 4, 5, 2
    isel 3, 0, 5, 9
    nop
    mr 3, 4
    pld 3, 8(4), 0
    pld 3, 8(0), 1
    paddi 3, 0, 100, 1
    pli 3, -5
    sync
    lwsync
    trap
    tdi 0, 0, 0
    mfspr 3, 1
    mfxer 3
    mtcrf 128, 3
    mtocrf 1, 3
    mtocrf 128, 3
    mfocrf 3, 2
    dcbt 3, 4
    dcbt 3, 4, 16
    dcbt 3, 4, 8
    dcbtst 3, 4
    dcbtst 3, 4, 16
    dcbtst 3, 4, 2
    dcbf 3, 4
    dcbfl 3, 4
    dcbflp 3, 4
    dcbfps 3, 4
    dcbstps 3, 4
    dcbf 3, 4, 2
    hashst 5, -8(1)
    hashst 5, -512(1)
    hashchk 5, -16(1)
    lxvp 34, 32(3)
    stxvp 2, -32(3)
    lq 4, 16(3)
    lxv 34, 16(3)
    stxv 1, -16(3)
    xxsetaccz 2
    xvf32ger 1, 2, 3
    mtvsrd 34, 3
    mfvsrd 3, 34
"""


def encodings(crate):
    """Each instruction's (size, mask, value), from tables.rs."""
    src = open(os.path.join(crate, "src", "tables.rs"), encoding="utf-8").read()
    out = []
    for m in re.finditer(r"Inst \{ name: \"(\w+)\", size: (\d), space: \d+, mask: (0x[0-9a-f]+), value: (0x[0-9a-f]+), ", src):
        out.append((int(m[2]), int(m[3], 16), int(m[4], 16)))
    return out


def words(crate, rng):
    out = []
    for size, mask, value in encodings(crate):
        bits = 8 * size
        for _ in range(PER_INSTRUCTION):
            out.append((size, (value | (rng.getrandbits(bits) & ~mask)) & ((1 << bits) - 1)))
    for _ in range(RANDOM_WORDS):
        w = rng.getrandbits(32)
        # (Not a prefix: that's two words.)
        if w >> 26 != 1:
            out.append((4, w))
    for _ in range(RANDOM_PREFIXED):
        out.append((8, (1 << 58) | rng.getrandbits(58)))
    return list(dict.fromkeys(out))


def edges(llvm, tmp):
    """EDGES' encodings: assembled (big-endian), read back word by word (a
    prefix and its suffix as one)."""
    asm = os.path.join(tmp, "edges.s")
    obj = os.path.join(tmp, "edges.o")
    raw = os.path.join(tmp, "edges.bin")
    open(asm, "w").write("\t.text\n" + EDGES)
    subprocess.run(
        [os.path.join(llvm, "llvm-mc"), "-triple", "powerpc64", "-mcpu=future", "-filetype=obj", asm, "-o", obj],
        check=True,
    )
    subprocess.run(
        [os.path.join(llvm, "llvm-objcopy"), "-O", "binary", "--only-section=.text", obj, raw],
        check=True,
    )
    b = open(raw, "rb").read()
    out = []
    at = 0
    while at + 4 <= len(b):
        w = int.from_bytes(b[at : at + 4], "big")
        if w >> 26 == 1 and at + 8 <= len(b):
            out.append((8, w << 32 | int.from_bytes(b[at + 4 : at + 8], "big")))
            at += 8
        else:
            out.append((4, w))
            at += 4
    return out


def disassemble(llvm, triple, ws, tmp):
    asm = os.path.join(tmp, "in.s")
    obj = os.path.join(tmp, "in.o")
    with open(asm, "w") as f:
        f.write("\t.text\n")
        for size, w in ws:
            if size == 8:
                f.write(f"\t.long {w >> 32:#x}\n\t.long {w & 0xFFFFFFFF:#x}\n")
            else:
                f.write(f"\t.long {w:#x}\n")
    subprocess.run(
        [os.path.join(llvm, "llvm-mc"), "-triple", triple, "-filetype=obj", asm, "-o", obj],
        check=True,
    )
    text = subprocess.run(
        [os.path.join(llvm, "llvm-objdump"), "-d", "--no-show-raw-insn", obj],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    out = {}
    for line in text.splitlines():
        m = re.match(r"\s*([0-9a-f]+):\s*(.*)$", line)
        if not m:
            continue
        rest = m[2].strip()
        # (A branch target's symbol: dropped.)
        rest = rest if rest.startswith("<") else re.sub(r"\s+<[^>]*>$", "", rest)
        out[int(m[1], 16)] = rest
    return out


def main():
    llvm, crate = sys.argv[1], sys.argv[2]
    rng = random.Random(20261121)
    ws = words(crate, rng)
    with tempfile.TemporaryDirectory() as tmp:
        ws = list(dict.fromkeys(ws + edges(llvm, tmp)))
    lines = [
        "# Generated by tools/powerpc/oracle.py: LLVM's text (llvm-objdump -d)",
        "# for encodings of each instruction and random words, at their addresses.",
        "# Mode, size in bytes, encoding (a prefixed one's prefix first), address,",
        "# then LLVM's text.",
    ]
    for mode, triple in (("PPC64LE", "powerpc64le"), ("PPC64", "powerpc64"), ("PPC32", "powerpc")):
        with tempfile.TemporaryDirectory() as tmp:
            got = disassemble(llvm, triple, ws, tmp)
        at = 0
        for size, w in ws:
            text = got.get(at, "<missing>")
            if size == 8 and text == "<unknown>":
                # (Not a prefixed instruction: LLVM decodes its words
                # alone, the prefix unknown, then its suffix.)
                text = "<unknown>"
            lines.append(f"{mode} {size} {w:0{size * 2}x} {at:x}\t{text}")
            at += size
    path = os.path.join(crate, "tests", "golden.txt")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w", newline="\n").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
