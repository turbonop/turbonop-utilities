"""Writes crates/isa/mips/tests/golden.txt: LLVM's text (llvm-objdump -d)
for encodings of each MIPS instruction hexcavator's decoder knows (up to four
of each, their operand bits random), and for random words, in several
configurations (an ISA level, a byte order, and the ASEs llvm-objdump is
given with --mattr); `<unknown>` where LLVM decodes nothing. And
golden-micromips.txt, the same of microMIPS's instructions (micromips.rs)
and random halfwords, each item two halfwords (a 16-bit instruction's
second a `move16 $0, $0`), then two more of those, so that whatever the
first two are, the next item starts where LLVM's next instruction does.

Usage: python oracle.py <LLVM bin directory> <crates/isa/mips>

The encodings come from the crate's tables.rs (each instruction's fixed
bits); the words are assembled with `.word` (llvm-mc, its -mcpu setting
the ELF flags llvm-objdump reads the ISA level from) and disassembled at
their addresses. Seeded: the same tables give the same file.
"""

import os
import random
import re
import subprocess
import sys
import tempfile

PER_INSTRUCTION = 4
RANDOM_WORDS = 3000

# name: (triple, -mcpu, objdump's --mattr). The golden test gives each
# name's features: the default CPU's, the ISA level's, and these.
CONFIGS = [
    ("MIPS32R2", "mips", "mips32r2", ""),
    ("MIPS64R2EL", "mips64el", "mips64r2", ""),
    ("MIPS32R6EL", "mipsel", "mips32r6", ""),
    ("MIPS64R6", "mips64", "mips64r6", ""),
    ("MIPS32R2EL+ASE", "mipsel", "mips32r2", "+msa,+dsp,+dspr2,+virt,+mt,+eva,+mcu"),
    ("MIPS64R6+ASE", "mips64", "mips64r6", "+msa,+dsp,+dspr2,+dspr3,+virt,+mt,+eva,+crc,+ginv"),
    ("OCTEON", "mips64", "mips64r2", "+cnmips,+cnmipsp"),
]


# Instructions random words rarely are: those LLVM's printer and decoder
# handle by hand (its aliases, `rdhwr`, `dext` and `dins`, Release 6's
# compact branches), assembled for each ISA level (-mcpu: its source).
EDGES = [
    ("mips32r2", """
        .set noreorder
        .set noat
        beq $zero, $zero, 1f
        beq $4, $zero, 1f
        bne $5, $zero, 1f
        beq $4, $5, 1f
    1:  bgezal $zero, 1b
        bgezal $4, 1b
        bc1t 1b
        bc1t $fcc1, 1b
        bc1f 1b
        jalr $25
        jalr $zero, $25
        jalr $4, $25
        jr $ra
        nor $2, $3, $zero
        nor $2, $3, $4
        or $2, $3, $zero
        addu $2, $3, $zero
        move $2, $3
        li $2, 5
        lui $1, 0x8000
        rdhwr $3, $29
        j 0x400100
        jal 0x7fffffc
        mfc0 $2, $12, 0
        mtc0 $2, $12, 3
        ext $2, $3, 4, 8
        ins $2, $3, 4, 8
        ins $2, $3, 0, 32
        sync
        syscall
        break
        teq $2, $3
        div $zero, $4, $5
        c.eq.s $f0, $f1
        c.ult.d $fcc2, $f0, $f2
        movf $2, $3, $fcc1
        lwxc1 $f0, $4($5)
        ldc1 $f2, 8($sp)
        sdc1 $f4, -8($sp)
        sw $ra, 28($sp)
        lw $ra, 28($sp)
        addiu $sp, $sp, -32
    """),
    ("mips64r2", """
        .set noreorder
        dext $2, $3, 4, 8
        dext $2, $3, 4, 40
        dext $2, $3, 36, 8
        dins $2, $3, 4, 8
        dins $2, $3, 4, 40
        dins $2, $3, 36, 8
        daddiu $sp, $sp, -32
        ld $ra, 24($sp)
        sd $ra, 24($sp)
        dsll32 $2, $3, 4
        beq $4, $zero, .
        jalr $25
    """),
    ("mips64r6", """
        .set noreorder
        bovc $4, $3, .
        beqc $3, $4, .
        beqzalc $4, .
        bnvc $4, $3, .
        bnec $3, $4, .
        bnezalc $4, .
        blezc $4, .
        bgezc $4, .
        bgec $3, $4, .
        bgtzc $4, .
        bltzc $4, .
        bltc $3, $4, .
        blezalc $4, .
        bgezalc $4, .
        bgeuc $3, $4, .
        bgtzalc $4, .
        bltzalc $4, .
        bltuc $3, $4, .
        blez $4, .
        bgtz $4, .
        bc .
        balc .
        jic $4, 8
        jialc $4, 8
        aui $2, $3, 1
        dahi $2, $2, 1
        dati $2, $2, 1
        lsa $2, $3, $4, 2
        seleqz $2, $3, $4
        auipc $2, 1
        lwpc $2, 8
        jr $ra
    """),
]


def edges(llvm, tmp):
    """EDGES' encodings: each assembled (big-endian), read back word by
    word."""
    out = []
    for cpu, src in EDGES:
        asm = os.path.join(tmp, "edges.s")
        obj = os.path.join(tmp, "edges.o")
        raw = os.path.join(tmp, "edges.bin")
        open(asm, "w").write("\t.text\n" + src)
        triple = "mips" if cpu.startswith("mips32") else "mips64"
        subprocess.run(
            [os.path.join(llvm, "llvm-mc"), "-triple", triple, f"-mcpu={cpu}", "-filetype=obj", asm, "-o", obj],
            check=True,
        )
        subprocess.run(
            [os.path.join(llvm, "llvm-objcopy"), "-O", "binary", "--only-section=.text", obj, raw],
            check=True,
        )
        b = open(raw, "rb").read()
        out += [int.from_bytes(b[at : at + 4], "big") for at in range(0, len(b) - 3, 4)]
    return out


def encodings(crate):
    """Each instruction's (mask, value), from tables.rs."""
    src = open(os.path.join(crate, "src", "tables.rs"), encoding="utf-8").read()
    out = []
    for m in re.finditer(r"Inst \{ name: \"(\w+)\", size: 4, space: \d+, mask: (0x[0-9a-f]+), value: (0x[0-9a-f]+), ", src):
        out.append((int(m[2], 16), int(m[3], 16)))
    return out


def words(crate, rng):
    out = []
    for mask, value in encodings(crate):
        for _ in range(PER_INSTRUCTION):
            out.append((value | (rng.getrandbits(32) & ~mask)) & 0xFFFFFFFF)
    for _ in range(RANDOM_WORDS):
        out.append(rng.getrandbits(32))
    # (Not 0: llvm-objdump writes runs of zero words as `...`.)
    return [w for w in dict.fromkeys(out) if w != 0]


def disassemble(llvm, triple, cpu, mattr, ws, tmp):
    asm = os.path.join(tmp, "in.s")
    obj = os.path.join(tmp, "in.o")
    with open(asm, "w") as f:
        f.write("\t.text\n")
        for w in ws:
            f.write(f"\t.word {w:#x}\n")
    subprocess.run(
        [os.path.join(llvm, "llvm-mc"), "-triple", triple, f"-mcpu={cpu}", "-filetype=obj", asm, "-o", obj],
        check=True,
    )
    args = [os.path.join(llvm, "llvm-objdump"), "-d", "--no-show-raw-insn"]
    if mattr:
        args.append(f"--mattr={mattr}")
    text = subprocess.run(args + [obj], check=True, capture_output=True, text=True).stdout
    out = {}
    last = None
    for line in text.splitlines():
        m = re.match(r"\s*([0-9a-f]+):\s*(.*)$", line)
        if not m:
            # (A line more of the last instruction's: `rdhwr`'s `.set`s.)
            if last is not None and line.startswith("\t"):
                out[last] += " " + line.strip()
            continue
        rest = m[2].strip()
        # (A branch target's symbol: dropped.)
        rest = rest if rest.startswith("<") else re.sub(r"\s+<[^>]*>$", "", rest)
        last = int(m[1], 16)
        out[last] = rest
    return out


# microMIPS's: (name, triple, -mcpu, -mattr for both tools).
MM_CONFIGS = [
    ("MM32R2", "mips", "mips32r2", "+micromips"),
    ("MM32R2EL", "mipsel", "mips32r2", "+micromips"),
    ("MM32R6", "mips", "mips32r6", "+micromips"),
    ("MM32R6EL", "mipsel", "mips32r6", "+micromips"),
    ("MM32R2+ASE", "mipsel", "mips32r2", "+micromips,+dsp,+dspr2,+fp64,+virt,+mt,+eva"),
]
MM_NOP = 0x0C00


def mm_items(crate, rng):
    """Each item's two halfwords: encodings of each microMIPS instruction,
    then random ones."""
    src = open(os.path.join(crate, "src", "micromips.rs"), encoding="utf-8").read()
    out = []
    for m in re.finditer(r"Inst \{ name: \"(\w+)\", size: (\d), space: \d+, mask: (0x[0-9a-f]+), value: (0x[0-9a-f]+), ", src):
        size, mask, value = int(m[2]), int(m[3], 16), int(m[4], 16)
        bits = 8 * size
        for _ in range(PER_INSTRUCTION):
            v = (value | (rng.getrandbits(bits) & ~mask)) & ((1 << bits) - 1)
            out.append((v, MM_NOP) if size == 2 else (v >> 16, v & 0xFFFF))
    for _ in range(RANDOM_WORDS):
        out.append((rng.getrandbits(16), rng.getrandbits(16)))
    return list(dict.fromkeys(out))


def mm_disassemble(llvm, triple, cpu, mattr, items, tmp):
    asm = os.path.join(tmp, "in.s")
    obj = os.path.join(tmp, "in.o")
    with open(asm, "w") as f:
        f.write("\t.text\n\t.set micromips\n")
        for hi, lo in items:
            f.write(f"\t.hword {hi:#x}, {lo:#x}, {MM_NOP:#x}, {MM_NOP:#x}\n")
    subprocess.run(
        [os.path.join(llvm, "llvm-mc"), "-triple", triple, f"-mcpu={cpu}", f"-mattr={mattr}", "-filetype=obj", asm, "-o", obj],
        check=True,
    )
    args = [os.path.join(llvm, "llvm-objdump"), "-d", "--no-show-raw-insn", f"--mattr={mattr}"]
    text = subprocess.run(args + [obj], check=True, capture_output=True, text=True).stdout
    out = {}
    last = None
    for line in text.splitlines():
        m = re.match(r"\s*([0-9a-f]+):\s*(.*)$", line)
        if not m:
            if last is not None and line.startswith("\t"):
                out[last] += " " + line.strip()
            continue
        rest = m[2].strip()
        rest = rest if rest.startswith("<") else re.sub(r"\s+<[^>]*>$", "", rest)
        last = int(m[1], 16)
        out[last] = rest
    return out


def micromips(llvm, crate, rng):
    items = mm_items(crate, rng)
    lines = [
        "# Generated by tools/mips/oracle.py: LLVM's text (llvm-objdump -d) for",
        "# encodings of each microMIPS instruction and random halfwords, at",
        "# their addresses. Configuration (tools/mips/oracle.py's MM_CONFIGS),",
        "# the item's two halfwords, its address, then LLVM's text.",
    ]
    for name, triple, cpu, mattr in MM_CONFIGS:
        with tempfile.TemporaryDirectory() as tmp:
            got = mm_disassemble(llvm, triple, cpu, mattr, items, tmp)
        for k, (hi, lo) in enumerate(items):
            lines.append(f"{name} {hi:04x}{lo:04x} {8 * k:x}\t{got.get(8 * k, '<missing>')}")
    path = os.path.join(crate, "tests", "golden-micromips.txt")
    open(path, "w", newline="\n").write("\n".join(lines) + "\n")


def main():
    llvm, crate = sys.argv[1], sys.argv[2]
    micromips(llvm, crate, random.Random(20261122))
    rng = random.Random(20261121)
    ws = words(crate, rng)
    with tempfile.TemporaryDirectory() as tmp:
        ws = [w for w in dict.fromkeys(ws + edges(llvm, tmp)) if w != 0]
    lines = [
        "# Generated by tools/mips/oracle.py: LLVM's text (llvm-objdump -d) for",
        "# encodings of each instruction and random words, at their addresses.",
        "# Configuration (tools/mips/oracle.py's CONFIGS), word, address, then",
        "# LLVM's text.",
    ]
    for name, triple, cpu, mattr in CONFIGS:
        with tempfile.TemporaryDirectory() as tmp:
            got = disassemble(llvm, triple, cpu, mattr, ws, tmp)
        for k, w in enumerate(ws):
            lines.append(f"{name} {w:08x} {4 * k:x}\t{got.get(4 * k, '<missing>')}")
    path = os.path.join(crate, "tests", "golden.txt")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w", newline="\n").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
