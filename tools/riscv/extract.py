"""Writes crates/isa/riscv/src/tables.rs: RISC-V's instructions as LLVM's
TableGen describes them (`llvm-tblgen --dump-json` of
llvm/lib/Target/RISCV/RISCV.td; see tools/tablegen/tablegen.py): those of
the standard extensions (not vendors', not experimental ones) that LLVM's
disassembler decodes, its aliases, its compressed instructions' full
forms, and its CSRs' names.

Usage: python extract.py <riscv.json> <out tables.rs>
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tablegen"))

import tablegen  # noqa: E402
from tablegen import Skip  # noqa: E402


class RiscV(tablegen.Extractor):
    TOOL = "riscv/extract.py"
    SOURCE = "llvm/lib/Target/RISCV/RISCV.td"
    NAMESPACES = ["", "RV32Only", "Zfinx", "ZdinxRV32Only", "ZcOverlap"]
    # (X's file must be 0: the decoder's RVE limit is of it.)
    FILES = {"X": 0, "F": 1, "V": 2}
    # 32-bit: the opcode and funct3; 16-bit: the quadrant and funct3.
    KEY32 = [0, 1, 2, 3, 4, 5, 6, 12, 13, 14]
    KEY16 = [0, 1, 13, 14, 15]
    PRINTS = {
        "printCSRSystemRegister": "Csr",
        "printFenceArg": "Fence",
        "printFRMArg": "Frm",
        "printFRMArgLegacy": "FrmLegacy",
        "printFPImmOperand": "FpImm",
        "printZeroOffsetMemOp": "ZeroOffsetMem",
        "printVTypeI": "VTypeI",
        "printRegList": "RegList",
        "printStackAdj": "StackAdj",
        "printNegStackAdj": "NegStackAdj",
        "printVMaskReg": "VMask",
        "printVScaleReg": "VScale",
        "printRegReg": "RegReg",
    }

    def standard(self, feature_record):
        name = self.d[feature_record]["Name"]
        return not (
            name.startswith("experimental")
            or name.startswith("xqc")
            or (name.startswith("x") and name != "xlen")
            or "vendor" in feature_record.lower()
        )

    def reg_of(self, name):
        """A register record's (file, number): X (GPRs, their FP views
        in Zfinx, pairs by their first), F, V (groups by their first)."""
        m = re.fullmatch(r"X(\d+)(_[A-Z]+)?", name)
        if m:
            return ("X", int(m.group(1)))
        m = re.fullmatch(r"X(\d+)_X(\d+)", name)
        if m:
            return ("X", int(m.group(1)))
        m = re.fullmatch(r"F(\d+)_[HFDQ]", name)
        if m:
            return ("F", int(m.group(1)))
        m = re.fullmatch(r"V(\d+)(M\d)?", name)
        if m:
            return ("V", int(m.group(1)))
        if name in ("DUMMY_REG_PAIR_WITH_X0", "X0_Pair"):
            return ("X", 0)
        return None

    def reg_decoder(self, dec, rc):
        m = re.fullmatch(r"DecodeSimpleRegisterClass<RISCV::[A-Z]+(\d+)\w*, (\d+)(, .*)?>", dec)
        if m:
            return ("RegSimple", int(m.group(1)))
        m = re.fullmatch(r"DecodeSingleRegister<RISCV::[A-Z]+(\d+), \d+>", dec)
        if m:
            return ("RegSingle", int(m.group(1)))
        m = re.fullmatch(r"DecodeVectorRegisterClass<RISCV::\w+, 32, (\d+)>", dec)
        if m:
            return ("RegGroup", int(m.group(1)))
        table = {
            "DecodeGPRRegisterClass": ("RegSimple", 0),
            "DecodeGPRNoX0RegisterClass": ("RegSimple", 0),
            "DecodeGPRNoX2RegisterClass": ("RegSimple", 0),
            "DecodeGPRNoX31RegisterClass": ("RegSimple", 0),
            "DecodeGPRX1X5RegisterClass": ("RegSimple", 0),
            "DecodeGPRNoX0X2RegisterClass": ("RegSimple", 0),
            "DecodeGPRPairRegisterClass": ("RegPair", 0),
            "DecodeGPRPairNoX0RegisterClass": ("RegPair", 0),
            "DecodeGPRPairCRegisterClass": ("RegPair", 8),
            "DecodeGPRS07RegisterClass": ("RegS07", 0),
            "decodeVMaskReg": ("VMask", 0),
            "decodeMvsa01Rs2Operand": ("Mvsa01", 0),
        }
        if dec in table:
            return table[dec]
        raise Skip(f"register decoder {dec} ({rc})")

    def imm_decoder(self, dec, cls):
        pats = [
            (r"decodeUImmOperand<(\d+)>", lambda m: ("UImm", int(m[1]), 0)),
            (r"decodeUImmOperand<(\d+), (\d+)>", lambda m: ("UImm", int(m[1]), int(m[2]))),
            (r"decodeSImmOperand<(\d+)>", lambda m: ("SImm", int(m[1]), 0)),
            (r"decodeSImm12LoOperand", lambda m: ("SImm", 12, 0)),
            (r"decodeUImm20Operand", lambda m: ("UImm", 20, 0)),
            (r"decodeSImmNonZeroOperand<(\d+)>", lambda m: ("SImmNonZero", int(m[1]), 0)),
            (r"decodeUImmNonZeroOperand<(\d+)>", lambda m: ("UImmNonZero", int(m[1]), 0)),
            (r"decodeSImmOperandAndLslN<(\d+), (\d+)>", lambda m: ("SImmLsl", int(m[1]), int(m[2]))),
            (r"decodeUImmLog2XLenOperand", lambda m: ("Log2XLen", 6, 0)),
            (r"decodeUImmLog2XLenNonZeroOperand", lambda m: ("Log2XLenNonZero", 6, 0)),
            (r"decodeUImmPlus1Operand<(\d+)>", lambda m: ("UImmPlus1", int(m[1]), 1)),
            (r"decodeUImmPlus1Operand<(\d+), (\d+)>", lambda m: ("UImmPlus1", int(m[1]), int(m[2]))),
            (r"decodeCLUIImmOperand", lambda m: ("CLui", 6, 0)),
            (r"decodeFRMArg", lambda m: ("Frm", 3, 0)),
            (r"decodeZcmpRlist", lambda m: ("Rlist", 4, 0)),
            (r"decodeFixedImmOperand<(-?\d+)>", lambda m: ("Fixed", 0, int(m[1]))),
            (r"", lambda m: ("Raw", 0, 0)),
        ]
        for p, f in pats:
            m = re.fullmatch(p, dec)
            if m:
                return f(m)
        raise Skip(f"immediate decoder {dec} ({cls})")

    def more(self):
        self.csrs = self.sysregs()

    def sysregs(self):
        d = self.d
        out = []
        for n in d["!instanceof"].get("SysReg", []):
            r = d[n]
            if int(r.get("isAltName", 0)) or int(r.get("isDeprecatedName", 0)):
                continue
            enc = 0
            for i, b in enumerate(r["Encoding"]):
                enc |= b << i
            feats = re.findall(r"RISCV::(\w+)", r.get("FeaturesRequired", "") or "")
            try:
                idx = [self.features[f] for f in feats]
                if any(not self.standard(f) for f in feats):
                    continue
            except KeyError:
                continue
            out.append((enc, json.loads(r["Name"]) if r["Name"].startswith('"') else r["Name"], idx, int(r.get("isRV32Only", 0))))
        out.sort(key=lambda s: s[0])
        return out

    def emit_more(self, w):
        w("pub static CSRS: &[crate::Csr] = &[")
        for enc, name, feats, rv32 in self.csrs:
            w(f"    crate::Csr {{ encoding: {enc:#x}, name: {json.dumps(name)}, features: &{feats}, rv32_only: {str(bool(rv32)).lower()} }},")
        w("];")
        w("")


if __name__ == "__main__":
    tablegen.main(RiscV)
