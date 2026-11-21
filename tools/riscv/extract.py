"""Writes crates/isa/riscv/src/tables.rs: RISC-V's instructions as LLVM's
TableGen describes them (`llvm-tblgen --dump-json` of
llvm/lib/Target/RISCV/RISCV.td), for hexcavator-riscv's decoder and
printer:

- each instruction of the standard extensions (not vendors', not
  experimental ones) that LLVM's disassembler decodes: its encoding's fixed
  bits, its decoder table, the features it needs, its assembly syntax, and
  its operands (each one's fields, how it decodes, and how it prints);
- the features, and what each implies;
- the aliases LLVM prints instructions as (`InstAlias`, by priority), and
  the compressed instructions' full forms (`CompressPat`), which it prints
  compressed instructions as;
- the CSRs' names.

Usage: python extract.py <riscv.json> <out tables.rs>
"""

import json
import re
import sys

STD_NAMESPACES = ["", "RV32Only", "Zfinx", "ZdinxRV32Only", "ZcOverlap"]


def main():
    src, out = sys.argv[1], sys.argv[2]
    d = json.load(open(src, encoding="utf-8"))
    E = Extractor(d)
    E.run()
    open(out, "w", encoding="utf-8", newline="\n").write(E.emit())
    reasons = {}
    for n, why in E.skipped.items():
        reasons.setdefault(why.split(":")[0] if ":" in why else why, []).append(n)
    print(
        f"{len(E.insts)} instructions ({len(E.skipped)} left out), "
        f"{len(E.aliases)} aliases, {len(E.compress)} compressed forms, "
        f"{len(E.csrs)} CSRs",
        file=sys.stderr,
    )
    for why, ns in sorted(reasons.items(), key=lambda x: -len(x[1])):
        print(f"  {len(ns)}: {why} ({', '.join(ns[:4])})", file=sys.stderr)


def flatten_dag_defs(x, out):
    """The defs a predicate's condition dag names."""
    if isinstance(x, dict) and x.get("kind") == "dag":
        for a in x["args"]:
            flatten_dag_defs(a[0], out)
    elif isinstance(x, dict) and x.get("kind") == "def":
        out.append(x["def"])


class Extractor:
    def __init__(self, d):
        self.d = d
        self.features = {}  # record name -> index
        self.feature_list = []  # (name, record)
        for n in d["!instanceof"]["SubtargetFeature"]:
            r = d[n]
            if not n.startswith("Feature"):
                continue
            self.features[n] = len(self.feature_list)
            self.feature_list.append((r["Name"], n))
        self.preds = []  # condition tuples
        self.pred_index = {}
        self.kinds = []  # operand kinds
        self.kind_index = {}
        self.classes = []  # register classes: (name, file, mask)
        self.class_index = {}
        self.insts = []
        self.inst_index = {}

    # ---------------------------------------------------------- features

    def standard(self, feature_record):
        name = self.d[feature_record]["Name"]
        return not (
            name.startswith("experimental")
            or name.startswith("xqc")
            or (name.startswith("x") and name != "xlen")
            or "vendor" in feature_record.lower()
        )

    def cond(self, pred):
        """A predicate's condition, as ("all"|"any"|"none", [feature
        indexes]), or None if it isn't an assembler predicate; raises
        Skip if it names a non-standard feature."""
        r = self.d[pred]
        dag = r.get("AssemblerCondDag")
        if not dag or not r.get("AssemblerMatcherPredicate", 1):
            return None
        op = dag["operator"]["def"]
        feats = []
        neg = []
        for a in dag["args"]:
            v = a[0]
            if v.get("kind") == "dag":
                inner = v["operator"]["def"]
                names = []
                flatten_dag_defs(v, names)
                if inner in ("not", "none_of") and len(names) == 1:
                    neg.append(names[0])
                    continue
                if inner in ("all_of", "any_of") and len(names) == 1:
                    feats.append(names[0])
                    continue
                raise ValueError(f"{pred}: nested {inner}")
            feats.append(v["def"])
        for f in feats + neg:
            if f not in self.features:
                raise ValueError(f"{pred}: {f}")
        kind = {"all_of": "all", "any_of": "any", "none_of": "none"}[op]
        # (A feature that isn't standard is never on: an `any` of others
        # holds without it, an `all` never, a `none` or `not` always.)
        if kind == "any":
            feats = [f for f in feats if self.standard(f)]
            if not feats:
                raise Skip(pred)
        elif kind == "all":
            for f in feats:
                if not self.standard(f):
                    raise Skip(f)
        else:
            feats = [f for f in feats if self.standard(f)]
        neg = [f for f in neg if self.standard(f)]
        return (
            kind,
            tuple(self.features[f] for f in feats),
            tuple(self.features[f] for f in neg),
        )

    def predicate(self, preds):
        """The index of a conjunction of conditions."""
        conds = []
        for p in preds:
            c = self.cond(p["def"])
            if c is not None:
                conds.append(c)
        key = tuple(conds)
        if key not in self.pred_index:
            self.pred_index[key] = len(self.preds)
            self.preds.append(key)
        return self.pred_index[key]

    # ---------------------------------------------------------- registers

    @staticmethod
    def reg_of(name):
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

    def members(self, x):
        """The registers a member list dag names: a set of (file, n)."""
        d = self.d
        if isinstance(x, list):
            x = x[0]
        if isinstance(x, dict) and x.get("kind") == "def":
            name = x["def"]
            r = d[name]
            if "MemberList" in r:
                return self.members(r["MemberList"])
            reg = self.reg_of(name)
            return {reg} if reg else set()
        if isinstance(x, dict) and x.get("kind") == "dag":
            op = x["operator"]["def"]
            args = x["args"]
            if op == "add":
                s = set()
                for a in args:
                    s |= self.members(a[0])
                return s
            if op == "sub":
                s = self.members(args[0][0])
                for a in args[1:]:
                    s -= self.members(a[0])
                return s
            if op == "sequence":
                fmt, lo, hi = args[0][0], args[1][0], args[2][0]
                step = -1 if lo > hi else 1
                s = set()
                for i in range(lo, hi + step, step):
                    reg = self.reg_of(fmt.replace("%u", str(i)))
                    if reg:
                        s.add(reg)
                return s
            if op == "decimate":
                regs = sorted(self.members(args[0][0]), key=lambda r: r[1])
                return set(regs[:: args[1][0]])
            if op == "trunc":
                regs = self.ordered(args[0][0])
                return set(regs[: args[1][0]])
            if op in ("rotl", "rotr", "shl"):
                return self.members(args[0][0])
            raise ValueError(f"member list {op}")
        raise ValueError(f"member list {x}")

    def ordered(self, x):
        return sorted(self.members(x), key=lambda r: r[1])

    def reg_class(self, name):
        """The index of register class `name` (by its record), and its file."""
        if name in self.class_index:
            return self.class_index[name]
        regs = self.members({"kind": "def", "def": name})
        files = {f for f, _ in regs}
        if len(files) != 1:
            raise ValueError(f"register class {name}: files {files}")
        f = files.pop()
        mask = 0
        for _, n in regs:
            mask |= 1 << n
        idx = len(self.classes)
        self.classes.append((name, f, mask))
        self.class_index[name] = idx
        return idx

    # ------------------------------------------------------------ operands

    def operand_kind(self, cls):
        """The index of the operand kind of operand class `cls` (a register
        class, a register operand, or an immediate operand)."""
        if cls in self.kind_index:
            return self.kind_index[cls]
        d = self.d
        r = d[cls]
        supers = r.get("!superclasses", [])
        print_method = r.get("PrintMethod") or "printOperand"
        pcrel = r.get("OperandType") == "OPERAND_PCREL"
        dec = r.get("DecoderMethod", "") or ""
        reg = None
        if "RegisterClass" in supers or "RegisterOperand" in supers:
            rc = r["RegClass"]["def"] if "RegisterOperand" in supers else cls
            if not dec and rc != cls:
                dec = d[rc].get("DecoderMethod", "") or ""
            if not dec:
                dec = f"Decode{rc}RegisterClass"
            reg = self.reg_class(rc)
            decode = self.reg_decoder(dec, rc)
        else:
            decode = self.imm_decoder(dec, cls)
        kind = (cls, reg, decode, print_method, pcrel, self.imm_pred(cls))
        idx = len(self.kinds)
        self.kinds.append(kind)
        self.kind_index[cls] = idx
        return idx

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

    def imm_pred(self, cls):
        """What an alias or compressed operand of class `cls` holds, as
        (signed, bits, lsb zeros, nonzero), from its name; None: anything."""
        m = re.fullmatch(r"(bare_)?([su])imm(\d+)(_lsb(0+))?(nonzero)?(_\w+)?", cls)
        if not m:
            return None
        return (m[2] == "s", int(m[3]), len(m[5] or ""), bool(m[6]))

    # -------------------------------------------------------- instructions

    def run(self):
        d = self.d
        names = sorted(
            n
            for n in d["!instanceof"]["Instruction"]
            if not d[n].get("isPseudo")
            and not d[n].get("isCodeGenOnly")
            and not d[n].get("isAsmParserOnly")
            and d[n].get("Inst")
            and d[n].get("DecoderNamespace") in STD_NAMESPACES
            and d[n].get("Size") in (2, 4)
        )
        self.skipped = {}
        for n in names:
            try:
                self.add_inst(n)
            except Skip as s:
                self.skipped[n] = str(s)
        self.aliases = []
        for n in d["!instanceof"]["InstAlias"]:
            try:
                a = self.alias(n)
            except Skip:
                continue
            if a:
                self.aliases.append(a)
        # By priority, highest first; then as defined.
        self.aliases.sort(key=lambda a: (-a["priority"], a["order"]))
        self.compress = []
        for n in d["!instanceof"]["CompressPat"]:
            try:
                c = self.uncompress(n)
            except Skip:
                continue
            if c:
                self.compress.append(c)
        self.csrs = self.sysregs()

    def add_inst(self, n):
        d = self.d
        r = d[n]
        pred = self.predicate(r["Predicates"])
        bits = r["Inst"]
        width = r["Size"] * 8
        mask = value = 0
        fields = {}
        for i, b in enumerate(bits[:width]):
            if isinstance(b, int):
                mask |= 1 << i
                value |= b << i
            elif b["kind"] == "varbit":
                fields.setdefault(b["var"], []).append((i, b["index"]))
            elif b["kind"] == "var":
                fields.setdefault(b["var"], []).append((i, 0))
        ops = []
        constraints = r.get("Constraints") or ""
        tied = {}
        for c in constraints.split(","):
            c = c.strip()
            m = re.fullmatch(r"\$(\w+)\s*=\s*\$(\w+)", c)
            if m:
                tied[m[1]] = m[2]
                tied[m[2]] = m[1]
        args = r["OutOperandList"]["args"] + r["InOperandList"]["args"]
        names = [a[1] for a in args]
        for (cls, name) in args:
            cls = cls["def"]
            kind = self.operand_kind(cls)
            segs = []
            if name in fields:
                # (inst bit, field bit) pairs, as runs.
                pairs = sorted(fields[name], key=lambda p: p[1])
                for ib, fb in pairs:
                    if segs and segs[-1][0] + segs[-1][2] == ib and segs[-1][1] + segs[-1][2] == fb:
                        segs[-1][2] += 1
                    else:
                        segs.append([ib, fb, 1])
            tie = None
            if not segs and name in tied:
                tie = names.index(tied[name])
            elif not segs and isinstance(r.get(name), list) and all(isinstance(b, int) for b in r[name]):
                # (Its value set in the record, as a constant field:
                # FCVTMOD.W.D's rounding mode.)
                v = sum(b << i for i, b in enumerate(r[name]))
                segs = [(-1, v, 0)]
            elif not segs and self.kinds[kind][2][0] == "VMask":
                # (A carry-in or merge mask: v0, encoded in no field.)
                segs = [(-1, 0, 0)]
            elif not segs and self.kinds[kind][2][0] not in ("RegSingle", "Fixed"):
                raise Skip(f"{n}: operand {name} has no field")
            ops.append((name, kind, [tuple(s) for s in segs], tie))
        # Tied pairs (the later of each is the one dags leave out).
        ties = []
        for a, b in tied.items():
            if a in names and b in names and names.index(a) < names.index(b):
                ties.append((names.index(a), names.index(b)))
        self.inst_index[n] = len(self.insts)
        self.insts.append(
            {
                "name": n,
                "size": r["Size"],
                "space": STD_NAMESPACES.index(r["DecoderNamespace"]),
                "mask": mask,
                "value": value,
                "pred": pred,
                "asm": r["AsmString"],
                "ops": ops,
                "fixed": bin(mask).count("1"),
                "ties": ties,
                "defs": len(r["OutOperandList"]["args"]),
            }
        )

    # -------------------------------------------------------------- aliases

    def mi_ops(self, inst):
        """An instruction's operands as an alias's or pattern's dag lists
        them: each MI operand's index, those tied to an earlier one of the
        same class left out."""
        out = []
        i_ = self.insts[self.inst_index[inst]]
        ops = i_["ops"]
        later = {b for a, b in i_["ties"] if self.kinds[ops[a][1]][0] == self.kinds[ops[b][1]][0]}
        for i in range(len(ops)):
            if i not in later:
                out.append(i)
        return out

    def dag_operand(self, arg):
        """A dag argument: ("op", class, name), ("reg", (file, n)),
        ("noreg",), or ("imm", value)."""
        v, name = arg
        if isinstance(v, int):
            return ("imm", v)
        if isinstance(v, list) and all(isinstance(b, int) for b in v):
            return ("imm", sum(b << i for i, b in enumerate(v)))
        if isinstance(v, dict) and v.get("kind") == "def":
            dn = v["def"]
            if name is not None:
                return ("op", dn, name)
            if dn in ("NoRegister", "zero_reg"):
                return ("noreg",)
            reg = self.reg_of(dn)
            if reg is None:
                raise Skip(f"dag register {dn}")
            return ("reg", reg)
        raise Skip(f"dag argument {arg}")

    def alias(self, n):
        d = self.d
        r = d[n]
        prio = int(r["EmitPriority"])
        if prio < 1:
            return None
        res = r["ResultInst"]
        inst = res["operator"]["def"]
        if inst not in self.inst_index:
            return None
        pred = self.predicate(r["Predicates"])
        mi = self.mi_ops(inst)
        args = res["args"]
        if len(args) != len(mi):
            raise Skip(f"{n}: operand count")
        ops = self.insts[self.inst_index[inst]]["ops"]
        conds = []
        names = {}
        for a, i in zip(args, mi):
            o = self.dag_operand(a)
            if o[0] == "op":
                cls, name = o[1], o[2]
                r2 = d[cls]
                supers = r2.get("!superclasses", [])
                is_reg = "RegisterClass" in supers or "RegisterOperand" in supers
                if name in names:
                    if is_reg:
                        conds.append(("tied", i, names[name]))
                    continue
                names[name] = i
                pm = r2.get("PrintMethod") or ""
                names[name + "#print"] = pm if pm and pm != "printOperand" else ""
                names[name + "#pcrel"] = r2.get("OperandType") == "OPERAND_PCREL"
                if is_reg:
                    rc = r2["RegClass"]["def"] if "RegisterOperand" in supers else cls
                    conds.append(("class", i, self.reg_class(rc)))
                else:
                    p = self.imm_pred(cls)
                    if p and r2.get("MCOperandPredicate"):
                        conds.append(("immpred", i, p))
            elif o[0] == "reg":
                conds.append(("reg", i, o[1]))
            elif o[0] == "noreg":
                conds.append(("noreg", i))
            else:
                conds.append(("imm", i, o[1]))
        # The alias's text: its operands, by the MI operand each is.
        asm = r["AsmString"]
        nops = len(re.findall(r"\$\{?\w+", asm))
        inst_nops = len(re.findall(r"\$\{?\w+", self.insts[self.inst_index[inst]]["asm"]))
        if nops > inst_nops:
            return None
        refs = []

        def sub(m):
            name = m.group(1) or m.group(2)
            if name not in names:
                raise Skip(f"{n}: alias operand {name}")
            refs.append((names[name], names[name + "#print"], names[name + "#pcrel"]))
            return "$" + str(len(refs) - 1)

        text = re.sub(r"\$\{(\w+)\}|\$(\w+)", sub, asm)
        # (LLVM writes a tab after an alias's mnemonic, as after an
        # instruction's.)
        if "	" not in text:
            text = text.replace(" ", "	", 1)
        order = int(n.split("_")[-1]) if n.startswith("anonymous_") else 10**9
        return {
            "inst": self.inst_index[inst],
            "priority": prio,
            "order": order,
            "pred": pred,
            "conds": conds,
            "asm": text,
            "refs": refs,
        }

    def uncompress(self, n):
        d = self.d
        r = d[n]
        if int(r.get("isCompressOnly", 0)):
            return None
        full = r["Input"]["operator"]["def"]
        comp = r["Output"]["operator"]["def"]
        if full not in self.inst_index or comp not in self.inst_index:
            return None
        pred = self.predicate(r["Predicates"] + d[full]["Predicates"])
        cmi = self.mi_ops(comp)
        cops = self.insts[self.inst_index[comp]]["ops"]
        if len(r["Output"]["args"]) != len(cmi):
            raise Skip(f"{n}: compressed operands")
        conds = []
        source = {}
        for a, i in zip(r["Output"]["args"], cmi):
            o = self.dag_operand(a)
            if o[0] == "op":
                if o[2] in source:
                    conds.append(("tied", i, source[o[2]]))
                else:
                    source[o[2]] = i
            elif o[0] == "reg":
                conds.append(("reg", i, o[1]))
            elif o[0] == "imm":
                conds.append(("imm", i, o[1]))
        # Tied operands of the compressed instruction are equal (the
        # decoder makes them so).
        fops = self.insts[self.inst_index[full]]["ops"]
        fmi = self.mi_ops(full)
        if len(r["Input"]["args"]) != len(fmi):
            raise Skip(f"{n}: full operands")
        built = [None] * len(fops)
        for a, i in zip(r["Input"]["args"], fmi):
            o = self.dag_operand(a)
            if o[0] == "op":
                if o[2] not in source:
                    raise Skip(f"{n}: {o[2]}")
                built[i] = ("from", source[o[2]])
                r2 = d[o[1]]
                supers = r2.get("!superclasses", [])
                if "RegisterClass" in supers or "RegisterOperand" in supers:
                    rc = r2["RegClass"]["def"] if "RegisterOperand" in supers else o[1]
                    conds.append(("class", source[o[2]], self.reg_class(rc)))
                else:
                    # (The full instruction's operand's predicate: `c.addi`
                    # of 0 is a hint, not `addi`.)
                    p = self.imm_pred(o[1])
                    if p and r2.get("MCOperandPredicate"):
                        conds.append(("immpred", source[o[2]], p))
            elif o[0] == "reg":
                built[i] = ("reg", o[1])
            elif o[0] == "imm":
                built[i] = ("imm", o[1])
            else:
                built[i] = ("noreg",)
        for i, (name, kind, segs, tie) in enumerate(fops):
            if built[i] is None:
                if tie is None:
                    raise Skip(f"{n}: full operand {name}")
                built[i] = built[tie]
        return {
            "comp": self.inst_index[comp],
            "full": self.inst_index[full],
            "pred": pred,
            "conds": conds,
            "built": built,
        }

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

    # ----------------------------------------------------------------- emit

    def emit(self):
        L = []
        w = L.append
        w("// Generated by tools/riscv/extract.py from LLVM's TableGen records")
        w("// (llvm-tblgen --dump-json of llvm/lib/Target/RISCV/RISCV.td).")
        w("// Don't edit: regenerate.")
        w("")
        w("#![allow(clippy::unreadable_literal)]")
        w("")
        w("use crate::tables_types::*;")
        w("")
        w(f"pub static FEATURES: &[Feature] = &[")
        for name, rec in self.feature_list:
            implies = [self.features[i["def"]] for i in self.d[rec].get("Implies", []) if i["def"] in self.features]
            w(f"    Feature {{ name: {json.dumps(name)}, implies: &{implies} }},")
        w("];")
        w("")
        w("pub static PREDICATES: &[&[Cond]] = &[")
        for p in self.preds:
            parts = []
            for kind, feats, neg in p:
                k = {"all": "All", "any": "Any", "none": "None"}[kind]
                parts.append(f"Cond {{ kind: CondKind::{k}, features: &{list(feats)}, not: &{list(neg)} }}")
            w("    &[" + ", ".join(parts) + "],")
        w("];")
        w("")
        w("pub static CLASSES: &[Class] = &[")
        for name, f, mask in self.classes:
            w(f"    Class {{ name: {json.dumps(name)}, file: File::{f}, members: {mask:#x} }},")
        w("];")
        w("")
        w("pub static KINDS: &[Kind] = &[")
        for cls, reg, dec, pm, pcrel, ipred in self.kinds:
            dk = dec[0]
            reg_s = f"Some({reg})" if reg is not None else "None"
            if dk.startswith("Reg") or dk in ("VMask", "Mvsa01"):
                dec_s = f"Decode::{dk}({dec[1]})"
            elif dk in ("Fixed",):
                dec_s = f"Decode::Fixed({dec[2]})"
            else:
                dec_s = f"Decode::{dk} {{ bits: {dec[1]}, arg: {dec[2]} }}"
            pm_s = {
                "printOperand": "Print::Operand",
                "printBranchOperand": "Print::Branch",
                "printCSRSystemRegister": "Print::Csr",
                "printFenceArg": "Print::Fence",
                "printFRMArg": "Print::Frm",
                "printFRMArgLegacy": "Print::FrmLegacy",
                "printFPImmOperand": "Print::FpImm",
                "printZeroOffsetMemOp": "Print::ZeroOffsetMem",
                "printVTypeI": "Print::VTypeI",
                "printRegList": "Print::RegList",
                "printStackAdj": "Print::StackAdj",
                "printNegStackAdj": "Print::NegStackAdj",
                "printVMaskReg": "Print::VMask",
                "printVScaleReg": "Print::VScale",
                "printRegReg": "Print::RegReg",
            }.get(pm)
            if pm_s is None:
                raise ValueError(f"print method {pm} ({cls})")
            w(f"    Kind {{ name: {json.dumps(cls)}, class: {reg_s}, decode: {dec_s}, print: {pm_s} }},")
        w("];")
        w("")
        w("pub static INSTRUCTIONS: &[Inst] = &[")
        for i in self.insts:
            ops = []
            for name, kind, segs, tie in i["ops"]:
                const = "None"
                if segs and segs[0][0] == -1:
                    const = f"Some({segs[0][1]})"
                    segs = []
                segs_s = ", ".join(f"({a}, {b}, {c})" for a, b, c in segs)
                tie_s = f"Some({tie})" if tie is not None else "None"
                ops.append(
                    f"Op {{ name: {json.dumps(name)}, kind: {kind}, fields: &[{segs_s}], "
                    f"constant: {const}, tied: {tie_s} }}"
                )
            w(
                f"    Inst {{ name: {json.dumps(i['name'])}, size: {i['size']}, space: {i['space']}, "
                f"mask: {i['mask']:#x}, value: {i['value']:#x}, pred: {i['pred']}, defs: {i['defs']}, "
                f"asm: {json.dumps(i['asm'])}, ops: &[{', '.join(ops)}] }},"
            )
        w("];")
        w("")

        def cond_s(c):
            if c[0] == "tied":
                return f"Check::Tied({c[1]}, {c[2]})"
            if c[0] == "class":
                return f"Check::Class({c[1]}, {c[2]})"
            if c[0] == "reg":
                return f"Check::Reg({c[1]}, Reg::{c[2][0]}({c[2][1]}))"
            if c[0] == "noreg":
                return f"Check::Reg({c[1]}, Reg::None)"
            if c[0] == "imm":
                return f"Check::Imm({c[1]}, {c[2]})"
            if c[0] == "immpred":
                s, b, z, nz = c[2]
                return f"Check::ImmFits({c[1]}, ImmRange {{ signed: {str(s).lower()}, bits: {b}, zeros: {z}, nonzero: {str(nz).lower()} }})"
            raise ValueError(c)

        # The instructions each value of an instruction's key bits can be
        # (32-bit: the opcode and funct3; 16-bit: the quadrant and funct3),
        # in the order they're tried: by decoder table, then the most fixed
        # bits first (the more specific encoding).
        def key_bits(size):
            return [0, 1, 2, 3, 4, 5, 6, 12, 13, 14] if size == 4 else [0, 1, 13, 14, 15]

        for size, label in ((4, "BUCKETS32"), (2, "BUCKETS16")):
            kb = key_bits(size)
            buckets = [[] for _ in range(1 << len(kb))]
            for idx, i in enumerate(self.insts):
                if i["size"] != size:
                    continue
                for key in range(1 << len(kb)):
                    ok = True
                    for j, bit in enumerate(kb):
                        if i["mask"] >> bit & 1 and (i["value"] >> bit & 1) != (key >> j & 1):
                            ok = False
                            break
                    if ok:
                        buckets[key].append(idx)
            w(f"pub static {label}: &[&[u16]] = &[")
            for b in buckets:
                b.sort(key=lambda x: (self.insts[x]["space"], -self.insts[x]["fixed"], x))
                w(f"    &{b},")
            w("];")
            w("")
        w("pub static ALIASES: &[Alias] = &[")
        for a in self.aliases:
            refs = ", ".join(
                f"Ref {{ op: {o}, print: {('Some(Print::' + {'printBranchOperand': 'Branch', 'printCSRSystemRegister': 'Csr', 'printFenceArg': 'Fence', 'printFRMArg': 'Frm', 'printFRMArgLegacy': 'FrmLegacy', 'printZeroOffsetMemOp': 'ZeroOffsetMem', 'printVMaskReg': 'VMask', 'printVTypeI': 'VTypeI', 'printRegList': 'RegList'}[p] + ')') if p else 'None'} }}"
                for o, p, _ in a["refs"]
            )
            w(
                f"    Alias {{ inst: {a['inst']}, pred: {a['pred']}, asm: {json.dumps(a['asm'])}, "
                f"checks: &[{', '.join(cond_s(c) for c in a['conds'])}], refs: &[{refs}] }},"
            )
        w("];")
        w("")
        w("pub static UNCOMPRESS: &[Uncompress] = &[")
        for c in self.compress:
            built = []
            for b in c["built"]:
                if b[0] == "from":
                    built.append(f"Build::From({b[1]})")
                elif b[0] == "reg":
                    built.append(f"Build::Reg(Reg::{b[1][0]}({b[1][1]}))")
                elif b[0] == "imm":
                    built.append(f"Build::Imm({b[1]})")
                else:
                    built.append("Build::Reg(Reg::None)")
            w(
                f"    Uncompress {{ from: {c['comp']}, to: {c['full']}, pred: {c['pred']}, "
                f"checks: &[{', '.join(cond_s(x) for x in c['conds'])}], build: &[{', '.join(built)}] }},"
            )
        w("];")
        w("")
        w("pub static CSRS: &[Csr] = &[")
        for enc, name, feats, rv32 in self.csrs:
            w(f"    Csr {{ encoding: {enc:#x}, name: {json.dumps(name)}, features: &{feats}, rv32_only: {str(bool(rv32)).lower()} }},")
        w("];")
        w("")
        return "\n".join(L)


class Skip(Exception):
    pass


if __name__ == "__main__":
    main()
