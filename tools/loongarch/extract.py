"""Writes crates/isa/loongarch/src/tables.rs: LoongArch's instructions as
LLVM's TableGen describes them (`llvm-tblgen --dump-json` of
llvm/lib/Target/LoongArch/LoongArch.td; see tools/tablegen/tablegen.py):
those LLVM's disassembler decodes (the base, F and D, LSX and LASX, LVZ,
LBT, and the LA v1.1 and v1.2 additions), and its aliases.

Usage: python extract.py <loongarch.json> <out tables.rs>
"""

import os
import re
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tablegen"))

import tablegen  # noqa: E402
from tablegen import Skip  # noqa: E402


class LoongArch(tablegen.Extractor):
    TOOL = "loongarch/extract.py"
    SOURCE = "llvm/lib/Target/LoongArch/LoongArch.td"
    NAMESPACES = [""]
    # GPRs, FPRs (both widths), LSX's and LASX's vectors, the condition
    # flags, the FP control and status registers, LBT's scratch registers.
    FILES = {"R": 0, "F": 1, "VR": 2, "XR": 3, "FCC": 4, "FCSR": 5, "SCR": 6}
    # The major opcode's bits (encodings are told apart from the top).
    KEY32 = [22, 23, 24, 25, 26, 27, 28, 29, 30, 31]
    KEY16 = []
    PRINTS = {
        "printAtomicMemOp": "AtomicMem",
    }

    def reg_of(self, name):
        """A register record's (file, number)."""
        m = re.fullmatch(r"(R|F|VR|XR|FCC|FCSR|SCR)(\d+)(_64)?", name)
        if m and (m[3] is None or m[1] == "F"):
            return (m[1], int(m[2]))
        return None

    def reg_decoder(self, dec, rc):
        table = {
            "DecodeGPRRegisterClass": ("RegSimple", 0),
            "DecodeGPRNoR0R1RegisterClass": ("RegSimple", 0),
            "DecodeFPR32RegisterClass": ("RegSimple", 0),
            "DecodeFPR64RegisterClass": ("RegSimple", 0),
            "DecodeCFRRegisterClass": ("RegSimple", 0),
            "DecodeFCSRRegisterClass": ("RegSimple", 0),
            "DecodeLSX128RegisterClass": ("RegSimple", 0),
            "DecodeLASX256RegisterClass": ("RegSimple", 0),
            "DecodeSCRRegisterClass": ("RegSimple", 0),
        }
        if dec in table:
            return table[dec]
        raise Skip(f"register decoder {dec} ({rc})")

    def imm_decoder(self, dec, cls):
        pats = [
            (r"decodeUImmOperand<(\d+)>", lambda m: ("UImmAdd", int(m[1]), 0)),
            (r"decodeUImmOperand<(\d+), (-?\d+)>", lambda m: ("UImmAdd", int(m[1]), int(m[2]))),
            (r"decodeSImmOperand<(\d+)>", lambda m: ("SImm", int(m[1]), 0)),
            # (Shifted left S, then sign-extended from N + S bits; a
            # branch's is relative to its address, as it prints.)
            (
                r"decodeSImmOperand<(\d+), (\d+)(, true)?>",
                lambda m: ("SImmLsl", int(m[1]) + int(m[2]), int(m[2])),
            ),
            (r"", lambda m: ("Raw", 0, 0)),
        ]
        for p, f in pats:
            m = re.fullmatch(p, dec)
            if m:
                return f(m)
        raise Skip(f"immediate decoder {dec} ({cls})")

    def imm_pred(self, cls):
        # (`simm16_lsl2_br`: 18 bits, the low two zero; `uimm2_plus1`.)
        m = re.fullmatch(r"([su])imm(\d+)(_lsl(\d+))?(_\w+)?", cls)
        if not m:
            return None
        s = int(m[4] or 0)
        if m[5] and "plus1" in m[5]:
            return None
        return (m[1] == "s", int(m[2]) + s, s, False)

    def add_inst(self, n):
        super().add_inst(n)
        i = self.insts[-1]
        if n == "UD":
            # (Its operand is in rd and rj both, which must be equal; the
            # crate checks that, and decodes AMSWAP.W otherwise, as LLVM's
            # DecodeAMOrUDInstruction does. Its value: rd's.)
            name, kind, segs, tie = i["ops"][0]
            i["ops"][0] = (name, kind, [(0, 0, 5)], tie)


if __name__ == "__main__":
    tablegen.main(LoongArch)
