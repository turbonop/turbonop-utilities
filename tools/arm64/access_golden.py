# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# (Its expected output is LLVM's; see crates/isa/arm64/LICENSE-LLVM.)

"""Generates crates/isa/arm64/tests/data/access.txt: LLVM's view of what
each sample word reads and writes, the oracle for turbonop-arm64's access
data (tests/it/access.rs).

For each word (from `examples/access_sample.rs`), llvm-mc's disassembler
(--show-inst) gives its MCInst: the opcode and its operands, of which the
first NumDefs are written and the rest read (a tied operand, such as a
writeback base, is in both). MCInstrDesc's implicit defs and uses, and
whether it loads or stores, come from AArch64GenInstrInfo.inc and the
TableGen records. So the oracle shares LLVM's records with the access
data, but not the decoding: llvm-mc picks the record from the word, and
decodes the registers itself.

    python access_golden.py <words.txt> <llvm-mc> <llvm aarch64.json> <AArch64GenInstrInfo.inc> > access.txt

Each line: the word, LLVM's opcode, `w:` the registers written and `r:`
those read (by turbonop-arm64's register names: X0-X30, SP, V0-V31,
P0-P15, ZA, ...; the zero registers are left out), and `m:` load or
store.
"""

import json
import os
import re
import subprocess
import sys

CHUNK = 4096


def mcid(path):
    """NumDefs and load/store flags by opcode name."""
    out = {}
    row = re.compile(r"\{ \d+,\s*(\d+),\s*(\d+),.*?,\s*(0(?:\|\(1ULL<<MCID::\w+\))*), 0x[0-9a-fA-F]+ULL \},\s*// (\w+)")
    for line in open(path, encoding="utf-8"):
        m = row.search(line)
        if m:
            flags = set(re.findall(r"MCID::(\w+)", m.group(3)))
            out[m.group(4)] = (int(m.group(2)), flags)
    return out


def ids(name):
    """turbonop-arm64's names for the registers an LLVM register name
    covers (tuples such as Q0_Q1 or Z0_Z8 are several)."""
    out = []
    for part in name.split("_"):
        m = re.fullmatch(r"([A-Z]+)(\d*)", part)
        if not m:
            raise SystemExit(f"a register name this script doesn't know: {name}")
        kind, n = m.group(1), m.group(2)
        if kind in ("X", "W") and n:
            out.append(f"X{n}")
        elif kind in ("FP",):
            out.append("X29")
        elif kind in ("LR",):
            out.append("X30")
        elif kind in ("XZR", "WZR"):
            pass
        elif kind in ("SP", "WSP"):
            out.append("SP")
        elif kind in ("B", "H", "S", "D", "Q", "Z") and n:
            out.append(f"V{n}")
        elif kind in ("P", "PN") and n:
            out.append(f"P{n}")
        elif kind.startswith("ZA"):
            out.append("ZA")
        elif kind == "ZT" and n == "0":
            out.append("ZT0")
        elif kind in ("FFR", "NZCV", "FPCR", "FPSR", "FPMR", "VG"):
            out.append(kind)
        else:
            raise SystemExit(f"a register name this script doesn't know: {name}")
    return out


def main():
    words_path, mc, json_path, info_path = sys.argv[1:5]
    d = json.load(open(json_path, encoding="utf-8"))
    desc = mcid(info_path)
    words = [int(l, 16) for l in open(words_path) if l.strip()]
    out = []
    for start in range(0, len(words), CHUNK):
        chunk = words[start:start + CHUNK]
        text = "".join(
            ",".join(f"0x{(w >> s) & 0xFF:02x}" for s in (0, 8, 16, 24)) + "\n" for w in chunk
        )
        p = subprocess.run(
            [mc, "--disassemble", "-triple=aarch64", "--mattr=+all", "--show-inst"],
            input=text, capture_output=True, text=True,
        )
        # The lines llvm-mc rejects (its warnings name them); the rest
        # print an MCInst each, in order.
        bad = {int(m.group(1)) for m in re.finditer(r"<stdin>:(\d+):\d+: warning: invalid instruction encoding", p.stderr)}
        insts = []
        cur = None
        for line in p.stdout.splitlines():
            m = re.search(r"<MCInst #\d+ (\w+)", line)
            if m:
                cur = [m.group(1), []]
                insts.append(cur)
            m = re.search(r"<MCOperand (Reg|Imm|Expr|SFPImm|DFPImm|Inst):([^>]*)>", line)
            if m and cur is not None:
                cur[1].append((m.group(1), m.group(2)))
        good = [w for i, w in enumerate(chunk) if i + 1 not in bad]
        if len(good) != len(insts):
            raise SystemExit(f"llvm-mc's output doesn't line up with its input near word {start}")
        for w, (opcode, ops) in zip(good, insts):
            ndefs, flags = desc[opcode]
            r = d[opcode]
            writes, reads = [], []
            for i, (kind, v) in enumerate(ops):
                if kind != "Reg":
                    continue
                (writes if i < ndefs else reads).extend(ids(v))
            for x in r.get("Defs") or []:
                writes.extend(ids(x["def"]))
            for x in r.get("Uses") or []:
                reads.extend(ids(x["def"]))
            mem = [m for m, f in (("load", "MayLoad"), ("store", "MayStore")) if f in flags]

            def fmt(regs):
                seen = []
                for x in regs:
                    if x not in seen:
                        seen.append(x)
                return ",".join(seen) or "-"

            out.append(f"{w:08X} {opcode} w:{fmt(writes)} r:{fmt(reads)} m:{','.join(mem) or '-'}")
    print("# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception")
    print("# LLVM's view of each word's reads and writes: generated by tools/arm64/access_golden.py.")
    if os.environ.get("LLVM_REV"):
        print(f"# LLVM {os.environ['LLVM_REV']}.")
    print("\n".join(out))


if __name__ == "__main__":
    main()
