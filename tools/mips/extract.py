"""Writes crates/isa/mips/src/tables.rs: MIPS's instructions as LLVM's
TableGen describes them (`llvm-tblgen --dump-json` of
llvm/lib/Target/Mips/Mips.td; see tools/tablegen/tablegen.py): those
LLVM's disassembler decodes in its standard encoding (MIPS I to MIPS64
R6, with MSA, DSP, the virtualization and microcontroller extensions,
and Cavium's), its decoder tables in the order it tries them, each by the
features that enable it, and its aliases.

Usage: python extract.py <mips.json> <out tables.rs>
"""

import os
import re
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tablegen"))

import tablegen  # noqa: E402
from tablegen import Skip  # noqa: E402


class Mips(tablegen.Extractor):
    TOOL = "mips/extract.py"
    SOURCE = "llvm/lib/Target/Mips/Mips.td"
    # LLVM's decoder tables, in the order MipsDisassembler tries them
    # (microMIPS's aren't decoded here: its encodings are others').
    NAMESPACES = [
        "COP3_",
        "Mips32r6_64r6_GP64",
        "Mips32r6_64r6_PTR64",
        "Mips32r6_64r6",
        "Mips32_64_PTR64",
        "CnMips",
        "CnMipsP",
        "Mips64",
        "MipsFP64",
        "Mips",
        # (Never decoded from: the instructions LLVM's decoder chooses by
        # hand among, Release 6's compact branches.)
        "Mips32r6_64r6_Ambiguous",
    ]
    # Each table's features: all of the first, none of the second (as
    # MipsDisassembler::getInstruction checks them).
    SPACES = {
        "COP3_": ([], ["mips32", "mips3"]),
        "Mips32r6_64r6_GP64": (["mips32r6", "gp64"], []),
        "Mips32r6_64r6_PTR64": (["mips32r6", "ptr64"], []),
        "Mips32r6_64r6": (["mips32r6"], []),
        "Mips32_64_PTR64": (["mips2", "ptr64"], []),
        "CnMips": (["cnmips"], []),
        "CnMipsP": (["cnmipsp"], []),
        "Mips64": (["gp64"], []),
        "MipsFP64": (["fp64"], []),
        "Mips": ([], []),
        "Mips32r6_64r6_Ambiguous": (["mips32r6"], ["mips32r6"]),
    }
    FILES = {
        "GPR": 0,
        "FPR": 1,
        "MSA": 2,
        "AC": 3,
        "HI": 4,
        "LO": 5,
        "FCC": 6,
        "FCR": 7,
        "COP0": 8,
        "COP2": 9,
        "COP3": 10,
        "HWR": 11,
        "MSACTRL": 12,
    }
    # The major opcode, and the low bits of the function field.
    KEY32 = [0, 1, 2, 3, 4, 5, 26, 27, 28, 29, 30, 31]
    KEY16 = []
    PRINTS = {
        "printUImm": "UImm",
        "printJumpOperand": "AbsBranch",
        "printMemOperand": "Mem",
        "printCOP0Operand": "Cop0",
        "printFCCOperand": "Fcc",
        "printOperandFromEnd": "FromEnd",
    }

    def print_name(self, method):
        m = re.fullmatch(r"printUImm<(\d+)(?:, (\d+))?>", method)
        if m:
            return f"UImm {{ bits: {m[1]}, offset: {m[2] or 0} }}"
        m = re.fullmatch(r"printOperandFromEnd<(\d+)>", method)
        if m:
            return f"FromEnd({m[1]})"
        return super().print_name(method)

    def reg_of(self, name):
        r = self.d.get(name)
        if not r or "HWEncoding" not in r:
            return None
        n = sum(b << i for i, b in enumerate(r["HWEncoding"]))
        if re.fullmatch(r"(ZERO|AT|V[01]|A[0-3]|T\d|S\d|K[01]|GP|SP|FP|RA)(_64)?", name):
            return ("GPR", n)
        if re.fullmatch(r"R\d+(_64)?", name):
            return ("GPR", n)
        if re.fullmatch(r"(F\d+|D\d+_64|D\d+|F_HI\d+)", name):
            return ("FPR", n)
        if re.fullmatch(r"W\d+", name):
            return ("MSA", n)
        if re.fullmatch(r"AC\d+(_64)?", name):
            return ("AC", n)
        if re.fullmatch(r"HI\d+(_64)?", name):
            return ("HI", n)
        if re.fullmatch(r"LO\d+(_64)?", name):
            return ("LO", n)
        if re.fullmatch(r"FCC\d+", name):
            return ("FCC", n)
        if re.fullmatch(r"FCR\d+", name):
            return ("FCR", n)
        if re.fullmatch(r"COP0\d+(_\d)?", name):
            # (Their encoding is a register and a select, reg << 5 | sel;
            # numbered as their class orders them: sel * 32 + reg.)
            return ("COP0", (n >> 5) + 32 * (n & 7))
        if re.fullmatch(r"COP2\d+", name):
            return ("COP2", n)
        if re.fullmatch(r"COP3\d+", name):
            return ("COP3", n)
        if re.fullmatch(r"HWR\d+", name):
            return ("HWR", n)
        if name.startswith("MSA") and name[3:4].isupper() or re.fullmatch(r"MSA\d+", name):
            return ("MSACTRL", n)
        return None

    def add_inst(self, n):
        r = self.d[n]
        if n.startswith("CRC32"):
            # (LLVM's DecodeCRC: rd, encoded in no field, is rt.)
            r = self.d[n] = dict(r, Constraints="$rd = $rt")
        if n.startswith("INSVE_"):
            # (LLVM's DecodeINSVE_DF: n2, encoded in no field, is 0.)
            r = self.d[n] = dict(r, n2=[0])
        super().add_inst(n)

    def reg_class_record(self, rc):
        # (Pointers: GPRs, 32- or 64-bit alike in number.)
        if rc in ("mips_ptr_rc", "ptr_sp_rc", "ptr_gp_rc"):
            return "GPR32"
        return rc

    def reg_decoder(self, dec, rc):
        m = re.fullmatch(r"DecodeFixedPtrRegister<(\d+)>", dec)
        if m:
            return ("RegSingle", int(m[1]))
        if dec == "DecodeHWRegsRegisterClass":
            # (Only $29.)
            return ("RegOnly", 29)
        if dec == "DecodeMSACtrlRegisterClass":
            # (8 of its 32.)
            return ("RegIndex", 8)
        if dec == "DecodeCOP0RegisterClass":
            return ("RegSelect", 0)
        if dec == "DecodeFGR64RegisterClass" and rc == "AFGR64":
            # (`ldc1`, `sdc1`: their AFGR64 operand decoded as FGR64.)
            return ("RegIndex", 0, "FGR64")
        if dec == "DecodeAFGR64RegisterClass":
            # (An even field: the pair whose first it is.)
            return ("RegPair", 0)
        if dec in ("DecodePtrRegisterClass",) or re.fullmatch(r"Decode\w+RegisterClass", dec):
            # (getReg: the class's register the field indexes.)
            return ("RegIndex", 0)
        raise Skip(f"register decoder {dec} ({rc})")

    def imm_decoder(self, dec, cls):
        if cls in ("jmptarget", "calltarget"):
            # (LLVM's DecodeJumpTarget: the instruction's 26 bits, << 2.)
            return ("UImmLsl", 26, 2)

        def scale(s):
            s = int(s or 1)
            k = s.bit_length() - 1
            if 1 << k != s:
                raise Skip(f"immediate decoder {dec} ({cls})")
            return k

        m = re.fullmatch(r"DecodeSImmWithOffsetAndScale<(\d+)(?:, (-?\d+))?(?:, (\d+))?>", dec)
        if m and int(m[2] or 0) == 0:
            k = scale(m[3])
            return ("SImmLsl", int(m[1]) + k, k)
        m = re.fullmatch(r"DecodeUImmWithOffsetAndScale<(\d+), (-?\d+), (\d+)>", dec)
        if m and int(m[2]) == 0:
            return ("UImmLsl", int(m[1]), scale(m[3]))
        m = re.fullmatch(r"DecodeUImmWithOffset<(\d+), (-?\d+)>", dec)
        if m:
            return ("UImmAdd", int(m[1]), int(m[2]))
        table = {
            "": ("Raw", 0, 0),
            # (A branch's offset is from the delay slot's address: + 4.)
            "DecodeBranchTarget": ("BranchPlus4", 18, 2),
            "DecodeBranchTarget21": ("BranchPlus4", 23, 2),
            "DecodeBranchTarget26": ("BranchPlus4", 28, 2),
            "DecodeSimm19Lsl2": ("SImmLsl", 21, 2),
            "DecodeSimm18Lsl3": ("SImmLsl", 21, 3),
            "DecodeSimm23Lsl2": ("SImmLsl", 25, 2),
            "DecodeInsSize": ("InsSize", 0, 2),
        }
        if dec in table:
            return table[dec]
        raise Skip(f"immediate decoder {dec} ({cls})")


    def more(self):
        # Each register's name, by file and number (as LLVM's printer
        # writes them: lowercase).
        self.names = {}
        for n in self.d["!instanceof"]["Register"]:
            reg = self.reg_of(n)
            if reg is None:
                continue
            name = (self.d[n].get("AsmName") or "").lower()
            if name:
                self.names.setdefault(reg, name)
        # The instructions with a delay slot (the instruction after them
        # runs before they take effect).
        self.delayed = [k for k, i in enumerate(self.insts) if int(self.d[i["name"]].get("hasDelaySlot", 0))]

    def emit_more(self, w):
        w("/// Each register's name, by file and number, as LLVM writes it.")
        w("pub static REG_NAMES: &[&[&str]] = &[")
        for f in sorted(self.FILES, key=lambda f: self.FILES[f]):
            ns = [n for (ff, n) in self.names if ff == f]
            top = max(ns) + 1 if ns else 0
            items = [self.names.get((f, i), "") for i in range(top)]
            w("    &[" + ", ".join(f'"{x}"' for x in items) + "],")
        w("];")
        w("")
        w("/// The instructions (by index) with a delay slot.")
        w(f"pub static DELAY_SLOT: &[u16] = &{self.delayed};")
        w("")


if __name__ == "__main__":
    tablegen.main(Mips)
