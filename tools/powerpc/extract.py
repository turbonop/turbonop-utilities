"""Writes crates/isa/powerpc/src/tables.rs: PowerPC's instructions as LLVM's
TableGen describes them (`llvm-tblgen --dump-json` of
llvm/lib/Target/PowerPC/PPC.td; see tools/tablegen/tablegen.py): those
LLVM's disassembler decodes (32- and 64-bit, Altivec, VSX, the prefixed
instructions of ISA 3.1, MMA, ...; not SPE's, whose encodings are
others'), and its aliases.

Usage: python extract.py <ppc.json> <out tables.rs>
"""

import os
import re
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tablegen"))

import tablegen  # noqa: E402
from tablegen import Skip  # noqa: E402


class PowerPc(tablegen.Extractor):
    TOOL = "powerpc/extract.py"
    SOURCE = "llvm/lib/Target/PowerPC/PPC.td"
    # (SPE's table is LLVM's only with SPE on: its encodings are
    # Altivec's.)
    NAMESPACES = [""]
    # GPRs (32- and 64-bit alike; `ZERO` is r0 read as 0); the 64 VSX
    # registers (the FPRs are 0 to 31, Altivec's 32 to 63); condition
    # register fields, and their bits; FPR pairs; VSX register pairs; MMA's
    # accumulators; dense math registers.
    FILES = {
        "R": 0,
        "VS": 1,
        "CR": 2,
        "CRBIT": 3,
        "FP": 4,
        "VSP": 5,
        "ACC": 6,
        "WACC": 7,
        "WACC_HI": 8,
        "DMR": 9,
        "DMRROWP": 10,
        "DMRP": 11,
        "DMRROW": 12,
    }
    # The primary opcode (bits 26-31) and the extended opcode's low bits;
    # a prefixed instruction's prefix's and suffix's primary opcodes.
    KEY32 = [1, 2, 3, 4, 5, 26, 27, 28, 29, 30, 31]
    KEY16 = []
    KEY64 = [26, 27, 28, 29, 30, 31, 58, 59, 60, 61, 62, 63]
    PRINTS = {
        "printUImmOperand": "Operand",
        "printSImmOperand": "Operand",
        "printS16ImmOperand": "Operand",
        "printU16ImmOperand": "Operand",
        "printS32ImmOperand": "Operand",
        "printNegS32ImmOperand": "Operand",
        "printS34ImmOperand": "Operand",
        "printImmZeroOperand": "Operand",
        "printU8ImmOperandTrunc": "Operand",
        "printAbsBranchOperand": "AbsBranch",
        "printcrbitm": "CrBitM",
        "printMemRegImm": "MemRegImm",
        "printMemRegImmHash": "MemRegImm",
        "printMemRegImm34": "MemRegImm",
        "printMemRegImm34PCRel": "MemRegImm",
        "printMemRegReg": "MemRegReg",
    }

    def print_name(self, method):
        # (`printUImmOperand<5>`: its template's arguments aside.)
        return super().print_name(re.sub(r"<.*>$", "", method))

    def reg_of(self, name):
        """A register record's (file, number): by its encoding, Altivec's
        (`V`, `VF`) past the FPRs'."""
        r = self.d.get(name)
        if not r or "HWEncoding" not in r:
            return None
        n = sum(b << i for i, b in enumerate(r["HWEncoding"]))
        m = re.fullmatch(r"([A-Za-z_]+?)(\d+)(LT|GT|EQ|UN)?", name)
        if name in ("ZERO", "ZERO8"):
            return ("R", 0)
        if not m:
            return None
        base = m[1]
        if base in ("R", "X"):
            return ("R", n)
        if base in ("F", "VSL", "VSX"):
            return ("VS", n)
        if base == "V":
            return ("VS", 32 + n)
        if base == "VF":
            # (VF's encoding is already VSX's.)
            return ("VS", n)
        if base == "CR" and m[3]:
            return ("CRBIT", n)
        if base == "CR":
            return ("CR", n)
        table = {
            "Fpair": "FP",
            "VSRp": "VSP",
            "ACC": "ACC",
            "UACC": "ACC",
            "WACC": "WACC",
            "WACC_HI": "WACC_HI",
            "DMR": "DMR",
            "DMRROWp": "DMRROWP",
            "DMRp": "DMRP",
            "DMRROW": "DMRROW",
        }
        if base in table:
            return (table[base], n)
        return None

    def reg_class_record(self, rc):
        # (GPR pairs decode as LLVM's GPR table does: any register.)
        if rc == "G8pRC":
            return "G8RC"
        # (The pointer classes, by hardware mode: 64-bit's.)
        r = self.d[rc]
        if "HwModeSelect" in r.get("!superclasses", []):
            objs = r.get("Objects") or []
            return objs[-1]["def"]
        return rc

    def reg_decoder(self, dec, rc):
        rc = self.reg_class_record(rc)
        if dec == "decodeVSRpEvenOperands":
            return ("RegHalf", 0)
        if dec == "DecodeFpRCRegisterClass":
            return ("RegPair", 0)
        if dec.startswith("Decode") and dec.endswith("RegisterClass") or dec == "":
            # (Each class's table: its field indexes its registers, which
            # are numbered from its first.)
            regs = self.members({"kind": "def", "def": rc})
            return ("RegSimple", min(n for _, n in regs))
        raise Skip(f"register decoder {dec} ({rc})")

    def imm_decoder(self, dec, cls):
        if cls == "dispRI":
            # (Its field as it is, which LLVM's printer reads as a short.)
            return ("SImm", 16, 0)
        pats = [
            (r"decodeUImmOperand<(\d+)>", lambda m: ("UImm", int(m[1]), 0)),
            (r"decodeSImmOperand<(\d+)>", lambda m: ("SImm", int(m[1]), 0)),
            (r"decodeCondBrTarget", lambda m: ("SImmLsl", 16, 2)),
            (r"decodeDirectBrTarget", lambda m: ("SImmLsl", 26, 2)),
            (r"decodeDispRIXOperand", lambda m: ("SImmLsl", 16, 2)),
            (r"decodeDispRIX16Operand", lambda m: ("SImmLsl", 16, 4)),
            (r"decodeDispRIHashOperand", lambda m: ("RiHash", 6, 0)),
            (r"decodeImmZeroOperand", lambda m: ("Zero", 0, 0)),
            (r"decodeCRBitMOperand", lambda m: ("CrBitM", self.FILES["CR"], 0)),
            (r"", lambda m: ("Raw", 0, 0)),
        ]
        for p, f in pats:
            m = re.fullmatch(p, dec)
            if m:
                return f(m)
        raise Skip(f"immediate decoder {dec} ({cls})")


    def more(self):
        # The features of the CPU llvm-objdump takes a PowerPC ELF file's
        # code to be (`future`'s: every one but SPE's and BookE's).
        d = self.d
        cpu = next(n for n in d["!instanceof"]["Processor"] if d[n]["Name"] == "future")
        self.future = [
            self.feature_list[self.features[f["def"]]][0]
            for f in d[cpu]["Features"]
            if f["def"] in self.features
        ]

    def emit_more(self, w):
        w("/// The features of LLVM's `future` CPU, which llvm-objdump takes PowerPC")
        w("/// code to be for.")
        w("pub static FUTURE: &[&str] = &[")
        for f in self.future:
            w(f'    "{f}",')
        w("];")
        w("")


if __name__ == "__main__":
    tablegen.main(PowerPc)
