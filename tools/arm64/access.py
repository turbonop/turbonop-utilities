# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# (Its operand and register data are LLVM's AArch64 target's; see crates/isa/arm64/LICENSE-LLVM.)

"""Generates crates/isa/arm64/tables/a64-access.txt: what each A64
encoding of tables/a64.txt reads and writes.

From LLVM's AArch64 target (`llvm-tblgen --dump-json` of
llvm/lib/Target/AArch64/AArch64.td), for the LLVM record each encoding's
operands name (their `src=llvm:<record>.<operand>` attributes):

- its operands, `(outs)` written and `(ins)` read, each with its bits and
  register class (an output tied to an input, `$Rn = $wback`, is both);
- its implicit registers (`Defs`, `Uses`: NZCV, LR, FFR, FPCR, ...);
- whether it loads, stores, calls, branches, or returns.

And from LLVM's disassembler (AArch64Disassembler.cpp), how each register
class's field decodes to registers: `class` lines list, for each field
value, the first register (and how many there are, and their stride).

What LLVM leaves out, or models only as `hasSideEffects`, is added here by
explicit rules (see `EXTRA`), each saying why: where unsure, more is
written, not less (a missing write misleads; an extra one only costs
precision).

Run it again whenever extract.py regenerates a64.txt:

    LLVM_REV=<commit> python access.py <a64.txt> <llvm aarch64.json> <AArch64GenInstrInfo.inc> > a64-access.txt

(AArch64GenInstrInfo.inc is in an LLVM build's
lib/Target/AArch64: MCInstrDesc's flags, which TableGen infers from the
patterns where the .td files don't say.)
"""

import json
import os
import re
import sys

# --------------------------------------------------------------------------
# Register classes: how a field's value decodes (LLVM's decoder for the
# class), as (file, element, count, stride, first(value) or None).
#
# Files: X and W (31 is the zero register), XSP and WSP (31 is the stack
# pointer), B H S D Q (scalar SIMD&FP), V64 V128 (vectors), Z, P, PN, ZA
# (the array, or a tile of it: the element says which size), ZT0.


def simple(n, first=0, mul=1, add=0, mod=None):
    def f(v):
        if v >= n:
            return None
        r = v * mul + add + first
        return r % mod if mod else r
    return f


def gpr_class(t, rc):
    """General-purpose register classes."""
    if t in ("GPR32sp",) or rc == "GPR32sp":
        return ("WSP", "", 1, 1, simple(32))
    if t.startswith("GPR32") or rc == "GPR32":
        return ("W", "", 1, 1, simple(32))
    if t in ("GPR64sp", "GPR64sp0") or rc == "GPR64sp":
        return ("XSP", "", 1, 1, simple(32))
    if t == "GPR64x8":
        # LD64B and ST64B: eight registers from an even one, up to X22.
        return ("X", "", 8, 1, lambda v: v if v <= 22 and v % 2 == 0 else None)
    if t in ("WSeqPairClassOperand",):
        # CASP: an even register and the next (W30 and WZR at 30).
        return ("W", "", 2, 1, lambda v: v if v % 2 == 0 else None)
    if t in ("XSeqPairClassOperand", "MrrsMssrPairClassOperand"):
        return ("X", "", 2, 1, lambda v: v if v % 2 == 0 else None)
    if t.startswith("GPR64") or rc in ("GPR64", "GPR64common"):
        # (GPR64common's decoder rejects 31; as the zero register, it
        # names nothing.)
        return ("X", "", 1, 1, simple(32))
    if t == "MatrixIndexGPR32Op8_11":
        return ("W", "", 1, 1, simple(4, first=8))
    if t == "MatrixIndexGPR32Op12_15":
        return ("W", "", 1, 1, simple(4, first=12))
    return None


ELEMENTS = {"8": "b", "16": "h", "32": "s", "64": "d", "128": "q"}


def element_of(t):
    """The element size letter of an SVE or SME register type, if any."""
    m = re.match(r"(?:ZPR|PPR|PNR|TileOp|MatrixOp|TileVectorOp[HV]|FPR)(8|16|32|64|128)", t)
    if m:
        return ELEMENTS[m.group(1)]
    m = re.match(r"(?:Z|ZZ|ZZZ|ZZZZ|PP)_([bhsdq])\b", t + " ")
    if m:
        return m.group(1)
    m = re.match(r"(?:ZZ|ZZZ|ZZZZ|PP)_([bhsdq])_", t)
    if m:
        return m.group(1)
    return ""


def register_class(t, d):
    """(file, element, count, stride, first) for register type `t`, or
    None if it isn't one."""
    r = d.get(t) or {}
    sup = r.get("!superclasses") or []
    if "RegisterClass" not in sup and "RegisterOperand" not in sup:
        return None
    rc = r.get("RegClass")
    rc = rc["def"] if isinstance(rc, dict) else t
    g = gpr_class(t, rc)
    if g:
        return g
    m = re.match(r"FPR(8|16|32|64|128)(Op|asZPR)?$", t)
    if m:
        return ("BHSDQ"["8 16 32 64 128".split().index(m.group(1))], "", 1, 1, simple(32))
    if t == "V64":
        return ("V64", "", 1, 1, simple(32))
    if t == "V128":
        return ("V128", "", 1, 1, simple(32))
    if t == "V128_lo":
        return ("V128", "", 1, 1, simple(16))
    if t == "V128_0to7":
        return ("V128", "", 1, 1, simple(8))
    m = re.match(r"VecList(One|Two|Three|Four)(\w+)$", t)
    if m:
        n = ["One", "Two", "Three", "Four"].index(m.group(1)) + 1
        arr = m.group(2)
        # (`VecListOneb`: a list of one 128-bit register, by its element.)
        wide = rc in ("FPR128", "QQ", "QQQ", "QQQQ")
        return ("V128" if wide else "V64", arr, n, 1, simple(32))
    el = element_of(t)
    # SVE vectors.
    if t in ("ZK", "ZK_b"):
        ks = [20, 21, 22, 23, 28, 29, 30, 31]
        return ("Z", el, 1, 1, lambda v: ks[v] if v < 8 else None)
    m = re.match(r"ZPR\d+Mul2_(Lo|Hi)$", t)
    if m:
        return ("Z", el, 1, 1, simple(8, mul=2, add=16 if m.group(1) == "Hi" else 0))
    if rc == "ZPR_3b":
        return ("Z", el, 1, 1, simple(8))
    if rc == "ZPR_4b":
        return ("Z", el, 1, 1, simple(16))
    if rc == "ZPR":
        return ("Z", el, 1, 1, simple(32))
    if rc == "ZPR2Strided":
        return ("Z", el, 2, 8, lambda v: (v & 7) + (v & 8) * 2 if v < 16 else None)
    if rc == "ZPR4Strided":
        return ("Z", el, 4, 4, lambda v: (v & 3) + (v & 4) * 4 if v < 16 else None)
    if rc == "ZPR2Mul2_Lo":
        return ("Z", el, 2, 1, simple(8, mul=2))
    if rc == "ZPR2Mul2_Hi":
        return ("Z", el, 2, 1, simple(8, mul=2, add=16))
    if rc == "ZPR2Mul2":
        return ("Z", el, 2, 1, simple(16, mul=2))
    if rc == "ZPR4Mul4":
        return ("Z", el, 4, 1, simple(8, mul=4))
    if rc == "ZPR3_3b":
        return ("Z", el, 3, 1, simple(8))
    for name, n in (("ZPR2", 2), ("ZPR3", 3), ("ZPR4", 4)):
        if rc == name:
            return ("Z", el, n, 1, simple(32))
    # Predicates.
    if rc == "PPR_3b":
        return ("P", el, 1, 1, simple(8))
    if rc in ("PPR", "PPRorPNR"):
        return ("P", el, 1, 1, simple(16))
    if rc == "PNR_p8to15":
        return ("PN", el, 1, 1, simple(8, first=8))
    if rc == "PNR":
        return ("PN", el, 1, 1, simple(16))
    if rc == "PPR2":
        return ("P", el, 2, 1, lambda v: v if v < 16 else None)
    if rc == "PPR2Mul2":
        return ("P", el, 2, 1, simple(8, mul=2))
    # SME: the ZA array (MatrixOp), its tiles (TileOp), and their slices
    # (TileVectorOp), all numbered by their field; ZT0.
    if rc == "MPR":
        return ("ZA", el, 1, 1, lambda v: 0)
    if rc.startswith("MPR"):
        # (MPR8 is ZAB0 alone: no field.)
        return ("ZA", el, 1, 1, lambda v: v)
    if t == "ZTR":
        return ("ZT0", "", 1, 1, lambda v: 0)
    return None


# --------------------------------------------------------------------------
# LLVM's records.


class Record:
    def __init__(self, name, r, d):
        self.name = name
        self.r = r
        bits = r.get("Inst") or []
        self.mask = self.value = 0
        # Each variable's bits, by its bit index: {var: {index: bit}}.
        self.var_bits = {}
        for i, b in enumerate(bits):
            if b in (0, 1):
                self.mask |= 1 << i
                self.value |= b << i
            elif isinstance(b, dict) and b.get("kind") in ("varbit", "var"):
                self.var_bits.setdefault(b["var"], {})[b.get("index", 0)] = i
        # The operands, in MC order (outs, then ins), complex ones expanded
        # into their parts: (name, type, is_out).
        self.ops = []
        for dag, out in (("OutOperandList", True), ("InOperandList", False)):
            for a in (r.get(dag) or {}).get("args", []):
                t, name = a
                if isinstance(t, dict) and t.get("kind") == "dag":
                    op = (t.get("operator") or {}).get("def")
                    info = ((d.get(op) or {}).get("MIOperandInfo") or {}).get("args", [])
                    for (sub, part) in zip(t.get("args", []), info):
                        self.ops.append((sub[1], part[0]["def"], out))
                    continue
                td = t.get("def")
                info = ((d.get(td) or {}).get("MIOperandInfo") or {}).get("args", [])
                if info:
                    for k, part in enumerate(info):
                        self.ops.append((f"{name}.{k}", part[0].get("def") if isinstance(part[0], dict) else None, out))
                else:
                    self.ops.append((name, td, out))
        # Tied operands: `$Rn = $wback`.
        self.tied = {}
        for c in (r.get("Constraints") or "").split(","):
            m = re.match(r"\s*\$(\w+)\s*=\s*\$(\w+)\s*$", c)
            if m:
                self.tied[m.group(1)] = m.group(2)
                self.tied[m.group(2)] = m.group(1)

    def bits(self, var):
        """The pieces (`hi:lo` or `b`, most significant first) of a
        variable's bits, or None."""
        vb = self.var_bits.get(var)
        if not vb:
            return None
        order = [vb[i] for i in sorted(vb, reverse=True)]
        pieces = []
        for b in order:
            if pieces and pieces[-1][1] == b + 1:
                pieces[-1][1] = b
            else:
                pieces.append([b, b])
        return ",".join(f"{h}:{l}" if h != l else f"{h}" for h, l in pieces)


def matching_record(records, mask, value, mnemonic):
    """For an encoding without LLVM operands (NOP, PACIASP, ...): the
    record whose fixed bits are the encoding's."""
    best = None
    for rec in records:
        if (rec.value ^ value) & rec.mask & mask:
            continue
        if rec.mask & ~mask:
            continue
        mn = re.match(r"[A-Za-z0-9.]*", rec.r.get("AsmString") or "").group(0).lower()
        key = (mn != mnemonic, -bin(rec.mask & mask).count("1"), rec.name)
        if best is None or key < best[0]:
            best = (key, rec)
    return best[1] if best else None


# --------------------------------------------------------------------------
# What LLVM leaves out.

ALL_Z = [f"Z{i}" for i in range(32)]
ALL_P = [f"P{i}" for i in range(16)]

# Extra implicit writes and reads, by LLVM record: (defs, uses, flags, why).
EXTRA = {
    # SMSTART and SMSTOP (MSR SVCRSM/SVCRZA/SVCRSMZA): entering or leaving
    # Streaming SVE mode sets every Z and P register and FFR to zero, and
    # enabling or disabling ZA zeroes it (and ZT0).
    "MSRpstatesvcrImm1": (ALL_Z + ALL_P + ["FFR", "ZA", "ZT0", "SVCR"], [], [],
                          "streaming mode and ZA changes zero the vector state"),
    # MSR (immediate) to the other PSTATE fields (DAIF, SPSel, PAN, UAO, DIT,
    # SSBS, TCO, ALLINT, PM): PSTATE. SPSel picks the stack pointer, so SP
    # changes too.
    # (Which field is in the word: the `msrimm` flag has the decoder add
    # SP for SPSel.)
    "MSRpstateImm1": (["PSTATE"], [], ["msrimm"], "PSTATE fields"),
    "MSRpstateImm4": (["PSTATE"], [], ["msrimm"], "PSTATE fields"),
    # Exception returns restore PSTATE (NZCV with it) from SPSR, and the
    # stack pointer of the level returned to.
    "ERET": (["NZCV", "PSTATE", "SP"], [], [], "restores PSTATE from SPSR_ELx"),
    "ERETAA": (["NZCV", "PSTATE", "SP"], ["LR", "SP"], [], "restores PSTATE from SPSR_ELx"),
    "ERETAB": (["NZCV", "PSTATE", "SP"], ["LR", "SP"], [], "restores PSTATE from SPSR_ELx"),
    "DRPS": (["NZCV", "PSTATE", "SP"], [], [], "restores PSTATE from SPSR_ELx"),
    # SYS (and its aliases AT, DC, IC, TLBI, ...) changes system state: AT
    # writes PAR_EL1, DC ZVA and DC GVA write memory, and DC IVAC may discard
    # what was written. (LLVM marks it as loading and storing.)
    "SYSxt": (["SYSREGS"], [], [], "system operations"),
    "SYSPxt": (["SYSREGS"], [], [], "system operations"),
    "SYSPxt_XZR": (["SYSREGS"], [], [], "system operations"),
    # (SYSL's aliases GCSPOPM and GCSSS2 load from and store to the GCS.)
    "SYSLxt": ([], ["SYSREGS"], ["load", "store"], "system operations"),
    # ZERO {<mask>}: the tiles in its mask (an immediate to LLVM).
    "ZERO_M": (["ZA"], [], [], "zeroes ZA tiles"),
    # GCS pushes and pops move GCSPR_EL0.
    "GCSPUSHM": (["SYSREGS"], ["SYSREGS"], [], "moves GCSPR_EL0"),
    "GCSPOPM": (["SYSREGS"], ["SYSREGS"], [], "moves GCSPR_EL0"),
    "GCSSS1": (["SYSREGS"], ["SYSREGS"], [], "switches GCS"),
    "GCSSS2": (["SYSREGS"], ["SYSREGS"], [], "switches GCS"),
    "GCSPUSHX": (["SYSREGS"], ["SYSREGS"], [], "moves GCSPR_ELx"),
    "GCSPOPX": (["SYSREGS"], ["SYSREGS"], [], "moves GCSPR_ELx"),
    "GCSPOPCX": (["SYSREGS"], ["SYSREGS"], [], "moves GCSPR_ELx"),
}


# What LLVM says that isn't so, by record name (a regular expression):
# (implicit registers, flags) to drop, and why. The oracle test
# (tests/it/access.rs) knows each.
DROP = [
    # "The MRS is set as a NZCV setting instruction. Not all MRS
    # instructions require doing this" (AArch64InstrFormats.td): none do.
    (r"MRS", ["NZCV"], [], "MRS writes only Xt"),
    # Load exclusives set the exclusive monitor (a side effect), but
    # store nothing.
    (r"LDA?X[RP][BHWX]?", [], ["store"], "a load exclusive stores nothing"),
]


# Operands LLVM has the wrong way round: (record, operand) -> access.
OPERAND_ACCESS = {
    # SYSL writes Xt (LLVM lists it among the inputs). (Read too: LLVM's
    # records for its aliases GCSPOPM and GCSSS2 tie it, and so read it.)
    ("SYSLxt", "Rt"): "rw",
}


# Hints LLVM decodes as HINT, which it marks as loading and storing (any
# hint may be allocated some day): those allocated touch no register or
# memory. (HINT #<imm> itself stays as LLVM has it.)
HINTS = {"YIELD", "WFE", "WFI", "SEV", "SEVL", "DGH", "ESB", "PSB", "TSB", "GCSB",
         "CSDB", "CLRBHB", "BTI", "STCPH", "SLBND", "NOP"}
HINT_RECORD = "HINT_allocated"


# MCInstrDesc's flags (AArch64GenInstrInfo.inc), as access flags. (The
# JSON's mayLoad and mayStore are only what the .td files say: TableGen
# infers the rest from the patterns, and only the generated table has it.)
MCID_FLAGS = {
    "MayLoad": "load",
    "MayStore": "store",
    "Call": "call",
    "Branch": "branch",
    "IndirectBranch": "indirect",
    "Return": "return",
    "Barrier": "barrier",
    "UnmodeledSideEffects": "side",
    "MayRaiseFPException": "fpexc",
    "Trap": "trap",
}

# Exception-generating instructions: what the handler does is the
# handler's (as a callee's is the callee's), but they are not plain
# instructions.
EXCEPTIONS = {"SVC", "HVC", "SMC", "BRK", "HLT", "UDF", "DCPS1", "DCPS2", "DCPS3", "TCANCEL"}


def mcid_flags(path):
    """Each record's MCInstrDesc flags and NumDefs, by name."""
    out = {}
    row = re.compile(r"\{ \d+,\s*(\d+),\s*(\d+),.*?,\s*(0(?:\|\(1ULL<<MCID::\w+\))*), 0x[0-9a-fA-F]+ULL \},\s*// (\w+)")
    for line in open(path, encoding="utf-8"):
        m = row.search(line)
        if m:
            flags = re.findall(r"MCID::(\w+)", m.group(3))
            out[m.group(4)] = ([MCID_FLAGS[f] for f in flags if f in MCID_FLAGS], int(m.group(2)))
    return out


def main():
    a64_path, json_path, info_path = sys.argv[1], sys.argv[2], sys.argv[3]
    d = json.load(open(json_path, encoding="utf-8"))
    mcid = mcid_flags(info_path)
    records = {}
    for name in d["!instanceof"]["Instruction"]:
        r = d[name]
        if r.get("isPseudo") or r.get("isCodeGenOnly") or r.get("Size") != 4:
            continue
        bits = r.get("Inst")
        if not isinstance(bits, list) or len(bits) != 32:
            continue
        records[name] = Record(name, r, d)

    # The encodings, in a64.txt's order, and the LLVM record each names.
    encs = []
    cur = None
    for line in open(a64_path, encoding="utf-8"):
        if line.startswith("enc "):
            cur = {"name": line.split()[1], "refs": {}, "mask": 0, "value": 0, "asm": ""}
            encs.append(cur)
        elif line.startswith("  bits ") and cur:
            for i, c in enumerate(line[7:].strip()):
                bit = 31 - i
                # (Should-be bits, `Z` and `O`, count as fixed: LLVM's
                # records fix them.)
                if c in "01ZO":
                    cur["mask"] |= 1 << bit
                    cur["value"] |= int(c in "1O") << bit
        elif line.startswith("  asm ") and cur:
            cur["asm"] = line[6:].strip()
        elif cur and "src=llvm:" in line:
            for m in re.finditer(r"src=llvm:([A-Za-z0-9_]+)\.\S*?:(\S+)", line):
                inst, t = m.group(1), m.group(2)
                # (Registers count most: a system operation's name comes
                # from MSR's records, its register from SYS's.)
                weight = 2 if register_class(t, d) else 1
                cur["refs"][inst] = cur["refs"].get(inst, 0) + weight

    out = []
    out.append("# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception")
    out.append("# What each A64 encoding reads and writes, generated by tools/arm64/access.py")
    out.append("# from LLVM's AArch64 target (Apache-2.0 WITH LLVM-exception; see LICENSE-LLVM).")
    if os.environ.get("LLVM_REV"):
        out.append(f"# LLVM {os.environ['LLVM_REV']}.")
    out.append("#")
    out.append("# class <type> <file> <element> <count> <stride> <first register by field value, - if invalid>")
    out.append("# inst <record> <flags>: then `op <name> <r|w|rw> <type> <bits>`, `def`, `use`")
    out.append("# enc <encoding> <record>: in tables/a64.txt's order")
    out.append("")

    chosen = []
    for e in encs:
        if e["refs"]:
            inst = max(e["refs"].items(), key=lambda kv: kv[1])[0]
        else:
            mnemonic = re.match(r"[A-Za-z0-9.]*", e["asm"]).group(0).lower()
            rec = matching_record(records.values(), e["mask"], e["value"], mnemonic)
            if rec is None:
                sys.exit(f"no LLVM record for {e['name']}")
            inst = rec.name
            if inst in ("HINT", "NOP") and e["name"].split("_")[0] in HINTS:
                inst = HINT_RECORD
        chosen.append((e["name"], inst))

    used_classes = {}
    inst_lines = []
    for inst in sorted({i for _, i in chosen}):
        if inst == HINT_RECORD:
            inst_lines.append(f"inst {inst} side")
            continue
        rec = records.get(inst)
        if rec is None:
            sys.exit(f"LLVM has no record {inst}")
        r = rec.r
        if inst not in mcid:
            sys.exit(f"{inst} isn't in {info_path}")
        flags = list(mcid[inst][0])
        if inst in EXCEPTIONS:
            flags.append("exception")
        defs = [x["def"] for x in r.get("Defs") or []]
        uses = [x["def"] for x in r.get("Uses") or []]
        if inst in EXTRA:
            xd, xu, xf, _why = EXTRA[inst]
            defs += [x for x in xd if x not in defs]
            uses += [x for x in xu if x not in uses]
            flags += [x for x in xf if x not in flags]
        for pattern, drop_defs, drop_flags, _why in DROP:
            if re.fullmatch(pattern, inst):
                defs = [x for x in defs if x not in drop_defs]
                flags = [x for x in flags if x not in drop_flags]
        # Calls through a register (BLR, BLRAA, ...) are indirect: LLVM
        # marks only branches so.
        if "call" in flags and "indirect" not in flags and any(
            register_class(t, d) for _, t, is_out in rec.ops if not is_out
        ):
            flags.append("indirect")
        # Floating-point operations that may raise exceptions (or read FPCR)
        # write FPSR's cumulative flags (LLVM doesn't model FPSR).
        if ("fpexc" in flags or "FPCR" in uses) and "FPSR" not in defs:
            defs.append("FPSR")
        # MSR and MRS (register): which system register is in the word
        # (NZCV, FPCR, FPSR, ... are worked out from it).
        if inst in ("MSR", "MSRR"):
            flags.append("msr")
        if inst in ("MRS", "MRRS"):
            flags.append("mrs")
        lines = [f"inst {inst} {','.join(flags) or '-'}"]
        # Registers: each operand once, tied pairs merged (an output with
        # no bits of its own takes its input's).
        seen = set()
        names = {n for n, _, _ in rec.ops}
        for name, t, is_out in rec.ops:
            if name in seen or t is None:
                continue
            cls = register_class(t, d)
            if cls is None:
                continue
            partner = rec.tied.get(name)
            access = "w" if is_out else "r"
            all_names = [name]
            if partner and partner in names:
                access = "rw"
                all_names.append(partner)
                seen.add(partner)
            seen.add(name)
            access = OPERAND_ACCESS.get((inst, name), access)
            bits = None
            for n in all_names:
                bits = bits or rec.bits(n)
            if bits is None and cls[0] not in ("ZA", "ZT0"):
                sys.exit(f"{inst}: no bits for register operand {name}")
            used_classes[t] = cls
            lines.append(f"  op {'/'.join(all_names)} {access} {t} {bits or '-'}")
        if defs:
            lines.append("  def " + " ".join(defs))
        if uses:
            lines.append("  use " + " ".join(uses))
        inst_lines += lines

    for t in sorted(used_classes):
        file, el, count, stride, first = used_classes[t]
        firsts = []
        for v in range(32):
            f = first(v)
            firsts.append("-" if f is None else str(f))
        # (Trailing invalid values are left out.)
        while firsts and firsts[-1] == "-":
            firsts.pop()
        out.append(f"class {t} {file} {el or '-'} {count} {stride} {','.join(firsts) or '0'}")
    out.append("")
    out += inst_lines
    out.append("")
    for name, inst in chosen:
        out.append(f"enc {name} {inst}")
    print("\n".join(out))


if __name__ == "__main__":
    main()
