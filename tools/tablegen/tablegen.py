"""Instruction tables from LLVM's TableGen records (`llvm-tblgen
--dump-json` of a target), for hexcavator-tablegen's decoder: each
instruction LLVM's disassembler decodes (of the decoder tables and
features a target's extractor takes), its encoding's fixed bits, its
decoder table, the features it needs, its syntax, and its operands (each
one's fields, how it decodes, how it prints); the features, and what each
implies; register classes, from their member lists; the aliases LLVM
prints instructions as (`InstAlias`, by priority); and compressed
instructions' full forms (`CompressPat`), which it prints them as.

A target's extractor (tools/riscv/extract.py, tools/loongarch/extract.py)
subclasses `Extractor`: its decoder tables (`NAMESPACES`), its register
files (`FILES`, and `reg_of`, which names a register record's file and
number), how its decoder methods decode (`reg_decoder`, `imm_decoder`),
its print methods (`PRINTS`), the bits its encodings are bucketed by
(`KEY16`, `KEY32`, `KEY64`), the register class a class chosen by
hardware mode is (`reg_class_record`), and what else it reads and writes
(`more`, `emit_more`).

An operand of several (a complex operand: PowerPC's `memri`, a
displacement and a base) is its sub-operands, each with its own fields;
the first is named for the whole and prints as it does (its print
method reads the rest).
"""

import json
import re
import sys


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
        # (A target whose encodings number their bits from the top, as
        # PowerPC's do: `Inst{0}` the most significant.)
        self.reversed_bits = any(
            int(d[n].get("isLittleEndianEncoding", 0)) for n in d["!instanceof"].get("InstrInfo", [])
        )

    # ---------------------------------------------------------- features

    # What a target's extractor says (see the module's docs).
    NAMESPACES = [""]
    FILES = {}
    KEY16 = []
    KEY32 = []
    KEY64 = []
    # Each decoder table's features, by its namespace: (all of these, none
    # of those), by name; a table not named here is always tried.
    SPACES = {}
    PRINTS = {}
    SOURCE = ""

    def standard(self, feature_record):
        """Whether a feature is one to decode with (not a vendor's, not
        experimental)."""
        name = self.d[feature_record]["Name"]
        return not name.startswith("experimental")

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
        # (A feature that isn't a `Feature*` record, PowerPC's `AIXOS`,
        # is a target's, never on here.)
        kind = {"all_of": "all", "any_of": "any", "none_of": "none"}[op]
        # (A feature that isn't standard is never on: an `any` of others
        # holds without it, an `all` never, a `none` or `not` always.)
        def standard(f):
            return f in self.features and self.standard(f)

        if kind == "any":
            # (`any_of (not X), Y` holds when X is never on: PowerPC's
            # `ModernAs`, for any OS but AIX.)
            if any(not standard(f) for f in neg):
                return None
            if neg:
                raise ValueError(f"{pred}: any_of with a negated feature")
            feats = [f for f in feats if standard(f)]
            if not feats:
                raise Skip(pred)
        elif kind == "all":
            for f in feats:
                if not standard(f):
                    raise Skip(f)
        else:
            feats = [f for f in feats if standard(f)]
        neg = [f for f in neg if standard(f)]
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

    def reg_of(self, name):
        """A register record's (file, number), or None."""
        raise NotImplementedError

    def members(self, x):
        """The registers a member list dag names: a set of (file, n)."""
        return set(self.member_list(x))

    def member_list(self, x):
        """The registers a member list dag names, in its order (TableGen's
        set operators': `add` concatenates, `sub` removes, `sequence`
        counts, `decimate`, `trunc`, `rotl`, `rotr`, and `shl` take and
        turn the list), each once: a list of (file, n)."""
        d = self.d
        if isinstance(x, list):
            x = x[0]
        if isinstance(x, dict) and x.get("kind") == "def":
            name = x["def"]
            r = d[name]
            if "MemberList" in r:
                return self.member_list(r["MemberList"])
            reg = self.reg_of(name)
            return [reg] if reg else []
        if isinstance(x, dict) and x.get("kind") == "dag":
            op = x["operator"]["def"]
            args = x["args"]
            if op == "add":
                out = []
                for a in args:
                    out += self.member_list(a[0])
            elif op == "sub":
                gone = set()
                for a in args[1:]:
                    gone |= self.members(a[0])
                out = [r for r in self.member_list(args[0][0]) if r not in gone]
            elif op == "and":
                keep = self.members(args[1][0])
                out = [r for r in self.member_list(args[0][0]) if r in keep]
            elif op == "sequence":
                fmt, lo, hi = args[0][0], args[1][0], args[2][0]
                step = -1 if lo > hi else 1
                out = []
                for i in range(lo, hi + step, step):
                    reg = self.reg_of(fmt.replace("%u", str(i)))
                    if reg:
                        out.append(reg)
            elif op == "decimate":
                out = self.member_list(args[0][0])[:: args[1][0]]
            elif op == "trunc":
                out = self.member_list(args[0][0])[: args[1][0]]
            elif op in ("rotl", "rotr"):
                regs = self.member_list(args[0][0])
                k = args[1][0] % len(regs) if regs else 0
                out = regs[k:] + regs[:k] if op == "rotl" else regs[len(regs) - k :] + regs[: len(regs) - k]
            elif op == "shl":
                out = self.member_list(args[0][0])[args[1][0] :]
            else:
                raise ValueError(f"member list {op}")
            seen = set()
            return [r for r in out if not (r in seen or seen.add(r))]
        raise ValueError(f"member list {x}")

    def ordered(self, x):
        return sorted(self.members(x), key=lambda r: r[1])

    def reg_class_record(self, rc):
        """The register class record `rc` is (a class chosen by hardware
        mode: the one to decode with)."""
        return rc

    def reg_class(self, name):
        """The index of register class `name` (by its record), and its file."""
        name = self.reg_class_record(name)
        if name in self.class_index:
            return self.class_index[name]
        regs = self.member_list({"kind": "def", "def": name})
        files = {f for f, _ in regs}
        if len(files) != 1:
            raise ValueError(f"register class {name}: files {files}")
        f = files.pop()
        mask = 0
        for _, n in regs:
            if n < 64:
                mask |= 1 << n
        idx = len(self.classes)
        self.classes.append((name, f, mask, [n for _, n in regs]))
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
        # (A pointer class, PointerLikeRegClass, is the register class the
        # target says: `reg_class_record`.)
        pointer = "PointerLikeRegClass" in supers or "RegClassByHwMode" in supers
        if "RegisterClass" in supers or "RegisterOperand" in supers or pointer:
            rc = r["RegClass"]["def"] if "RegisterOperand" in supers else cls
            rc = self.reg_class_record(rc)
            if not dec and rc != cls:
                dec = d[rc].get("DecoderMethod", "") or ""
            if not dec:
                dec = f"Decode{rc}RegisterClass"
            decode = self.reg_decoder(dec, rc)
            # (A decoder of another class than the operand's: MIPS's
            # `ldc1`, an AFGR64 operand decoded as FGR64.)
            if len(decode) == 3:
                rc = decode[2]
                decode = decode[:2]
            reg = self.reg_class(rc)
        else:
            decode = self.imm_decoder(dec, cls)
        kind = (cls, reg, decode, print_method, pcrel, self.imm_pred(cls))
        idx = len(self.kinds)
        self.kinds.append(kind)
        self.kind_index[cls] = idx
        return idx

    def sub_operands(self, cls):
        """A complex operand class's sub-operands' classes (none for a
        simple one)."""
        info = self.d[cls].get("MIOperandInfo") or {}
        return [a[0]["def"] for a in info.get("args", [])]

    def complex_kind(self, cls):
        """The kind of a complex operand's first sub-operand: decoded as
        that sub-operand's class, printed as the complex operand."""
        key = cls + "#"
        if key in self.kind_index:
            return self.kind_index[key]
        first = self.kinds[self.operand_kind(self.sub_operands(cls)[0])]
        print_method = self.d[cls].get("PrintMethod") or "printOperand"
        kind = (cls, first[1], first[2], print_method, first[4], first[5])
        idx = len(self.kinds)
        self.kinds.append(kind)
        self.kind_index[key] = idx
        return idx

    def reg_decoder(self, dec, rc):
        """A register decoder method's (kind, base)."""
        raise NotImplementedError

    def imm_decoder(self, dec, cls):
        """An immediate decoder method's (kind, bits, arg)."""
        raise NotImplementedError

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
            and d[n].get("DecoderNamespace") in self.NAMESPACES
            and d[n].get("Size") in (2, 4, 8)
        )
        self.skipped = {}
        for n in names:
            try:
                self.add_inst(n)
            except Skip as s:
                self.skipped[n] = str(s)
        self.aliases = []
        for n in d["!instanceof"].get("InstAlias", []):
            try:
                a = self.alias(n)
            except Skip:
                continue
            if a:
                self.aliases.append(a)
        # (Aliases of an instruction whose conditions are another's are
        # ambiguous: LLVM's AsmWriterEmitter prints neither, whatever
        # their text.)
        def conditions(a):
            return (a["inst"], a["pred"], tuple(tuple(c) for c in a["conds"]))

        seen = {}
        for a in self.aliases:
            seen[conditions(a)] = seen.get(conditions(a), 0) + 1
        self.aliases = [a for a in self.aliases if seen[conditions(a)] == 1]
        # By priority, highest first; then as defined.
        self.aliases.sort(key=lambda a: (-a["priority"], a["order"]))
        self.compress = []
        for n in d["!instanceof"].get("CompressPat", []):
            try:
                c = self.uncompress(n)
            except Skip:
                continue
            if c:
                self.compress.append(c)
        # Each decoder table's condition, as a predicate.
        self.space_preds = []
        for ns in self.NAMESPACES:
            all_of, none_of = self.SPACES.get(ns, ([], []))
            names = {name: i for i, (name, _) in enumerate(self.feature_list)}
            cond = (
                "all",
                tuple(names[f] for f in all_of),
                tuple(names[f] for f in none_of),
            )
            key = (cond,) if all_of or none_of else ()
            if key not in self.pred_index:
                self.pred_index[key] = len(self.preds)
                self.preds.append(key)
            self.space_preds.append(self.pred_index[key])
        self.more()

    def more(self):
        """What else the target reads (RISC-V: its CSRs)."""

    def add_inst(self, n):
        d = self.d
        r = d[n]
        pred = self.predicate(r["Predicates"])
        bits = r["Inst"]
        width = r["Size"] * 8
        bits = bits[:width]
        if self.reversed_bits:
            bits = bits[::-1]
        mask = value = 0
        fields = {}
        for i, b in enumerate(bits):
            if isinstance(b, int):
                mask |= 1 << i
                value |= b << i
            elif b is None:
                # (A bit the encoding leaves unset: any.)
                continue
            elif b["kind"] == "varbit":
                fields.setdefault(b["var"], []).append((i, b["index"]))
            elif b["kind"] == "var":
                fields.setdefault(b["var"], []).append((i, 0))
        ops = []
        constraints = r.get("Constraints") or ""
        tied = {}
        for c in constraints.split(","):
            c = c.strip()
            m = re.fullmatch(r"\$([\w.]+)\s*=\s*\$([\w.]+)", c)
            if m:
                tied[m[1]] = m[2]
                tied[m[2]] = m[1]
        # Each operand (a complex one's sub-operands: the first named for
        # it), its kind, and the field it's encoded in; and the slots
        # dags list them by (an MI operand index, and how many).
        flat = []
        slots = []
        # (A sub-operand is also `operand.sub`, by the complex class's
        # names for them: `$addr.reg`.)
        dotted = {}
        for (cls, name) in r["OutOperandList"]["args"] + r["InOperandList"]["args"]:
            if cls.get("kind") == "dag":
                cplx = cls["operator"]["def"]
                subs = self.sub_operands(cplx)
                sub_names = [a[1] for a in cls["args"]]
                if len(subs) != len(sub_names) or not subs:
                    raise Skip(f"{n}: complex operand {name}")
                slots.append((len(flat), len(subs)))
                info = (d[cplx].get("MIOperandInfo") or {}).get("args", [])
                for k, a in enumerate(info):
                    dotted[f"{name}.{a[1]}"] = len(flat) + k
                for k, (sc, sn) in enumerate(zip(subs, sub_names)):
                    kind = self.complex_kind(cplx) if k == 0 else self.operand_kind(sc)
                    flat.append((name if k == 0 else f"{name}.{sn}", kind, sn))
                continue
            cls = cls["def"]
            if cls == "variable_ops":
                # (Not an operand: a placeholder for any more.)
                continue
            subs = self.sub_operands(cls)
            if len(subs) > 1:
                raise Skip(f"{n}: complex operand {name} without its fields")
            slots.append((len(flat), 1))
            # (A complex operand of one: that one, printed as the whole.)
            kind = self.complex_kind(cls) if subs else self.operand_kind(cls)
            flat.append((name, kind, name))
        names = [f[0] for f in flat]
        # (Constraints name operands, and complex operands' sub-operands.)
        index_of = dict(dotted)
        for k, (name, _, field_name) in enumerate(flat):
            index_of.setdefault(field_name, k)
            index_of[name] = k
        for (name, kind, field_name) in flat:
            segs = []
            if field_name in fields:
                # (inst bit, field bit) pairs, as runs.
                pairs = sorted(fields[field_name], key=lambda p: p[1])
                for ib, fb in pairs:
                    if segs and segs[-1][0] + segs[-1][2] == ib and segs[-1][1] + segs[-1][2] == fb:
                        segs[-1][2] += 1
                    else:
                        segs.append([ib, fb, 1])
            tie = None
            if not segs and (name in tied or field_name in tied):
                other = tied.get(name) or tied[field_name]
                if other not in index_of:
                    raise Skip(f"{n}: tied to {other}")
                tie = index_of[other]
            elif not segs and isinstance(r.get(field_name), list) and all(isinstance(b, int) for b in r[field_name]):
                # (Its value set in the record, as a constant field:
                # FCVTMOD.W.D's rounding mode.)
                v = sum(b << i for i, b in enumerate(r[field_name]))
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
            if a in index_of and b in index_of and index_of[a] < index_of[b]:
                ties.append((index_of[a], index_of[b]))
        self.inst_index[n] = len(self.insts)
        self.insts.append(
            {
                "name": n,
                "size": r["Size"],
                "space": self.NAMESPACES.index(r["DecoderNamespace"]),
                "mask": mask,
                "value": value,
                "pred": pred,
                "asm": r["AsmString"],
                "ops": ops,
                "fixed": bin(mask).count("1"),
                "ties": ties,
                "slots": slots,
                "defs": sum(c for _, c in slots[: len(r["OutOperandList"]["args"])]),
            }
        )

    # -------------------------------------------------------------- aliases

    def mi_ops(self, inst):
        """An instruction's operands as an alias's or pattern's dag lists
        them: each one's first MI operand's index (a complex operand's
        sub-operands are one), those tied to an earlier one of the same
        class left out."""
        return [first for first, _ in self.mi_slots(inst)]

    def mi_slots(self, inst):
        """As `mi_ops`, each with how many MI operands it is."""
        out = []
        i_ = self.insts[self.inst_index[inst]]
        ops = i_["ops"]
        later = {b for a, b in i_["ties"] if self.kinds[ops[a][1]][0] == self.kinds[ops[b][1]][0]}
        for first, count in i_["slots"]:
            if first not in later:
                out.append((first, count))
        return out

    def dag_operand(self, arg):
        """A dag argument: ("op", class, name), ("reg", (file, n)),
        ("noreg",), or ("imm", value)."""
        v, name = arg
        if isinstance(v, int):
            return ("imm", v)
        if isinstance(v, list) and all(isinstance(b, int) for b in v):
            return ("imm", sum(b << i for i, b in enumerate(v)))
        if isinstance(v, dict) and v.get("kind") == "dag":
            raise Skip(f"dag argument {v.get('printable')}")
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
        slots = self.mi_slots(inst)
        mi = [first for first, _ in slots]
        counts = {first: count for first, count in slots}
        args = res["args"]
        if len(args) != len(mi):
            raise Skip(f"{n}: operand count")
        ops = self.insts[self.inst_index[inst]]["ops"]
        conds = []
        names = {}
        for a, i in zip(args, mi):
            o = self.dag_operand(a)
            if counts[i] > 1 and o[0] != "op":
                raise Skip(f"{n}: complex operand's value")
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
                if counts[i] > 1:
                    # (A complex operand, as the instruction prints it.)
                    names[name + "#print"] = ""
                elif is_reg:
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
        # (LLVM prints an alias only if it has no more operands than its
        # instruction, counting them as its AsmWriterEmitter does: by the
        # spaces and tabs in each's syntax.)
        def count(text):
            return text.count(" ") + text.count("\t")

        if count(asm) > count(self.insts[self.inst_index[inst]]["asm"]):
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

    # ----------------------------------------------------------------- emit

    # LLVM's printer methods, as hexcavator-tablegen's `Print` names them.
    BASE_PRINTS = {
        "printOperand": "Operand",
        "printBranchOperand": "Branch",
    }

    def print_name(self, method):
        p = dict(self.BASE_PRINTS, **self.PRINTS).get(method)
        if p is None:
            raise ValueError(f"print method {method}")
        return p

    def reg_s(self, reg):
        file, n = reg
        return f"Reg::new({self.FILES[file]}, {n})"

    def emit(self):
        L = []
        w = L.append
        w(f"// Generated by tools/{self.TOOL} from LLVM's TableGen records")
        w(f"// (llvm-tblgen --dump-json of {self.SOURCE}).")
        w("// Don't edit: regenerate.")
        w("")
        w("#![allow(clippy::unreadable_literal)]")
        w("")
        w("use hexcavator_tablegen::*;")
        w("")
        w("pub static FEATURES: &[Feature] = &[")
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
        for name, f, mask, order in self.classes:
            w(
                f"    Class {{ name: {json.dumps(name)}, file: {self.FILES[f]}, members: {mask:#x}, "
                f"order: &{order} }},"
            )
        w("];")
        w("")
        w("pub static KINDS: &[Kind] = &[")
        for cls, reg, dec, pm, pcrel, ipred in self.kinds:
            dk = dec[0]
            reg_s = f"Some({reg})" if reg is not None else "None"
            if dk.startswith("Reg") or dk in ("VMask", "Mvsa01", "CrBitM"):
                dec_s = f"Decode::{dk}({dec[1]})"
            elif dk in ("Fixed",):
                dec_s = f"Decode::Fixed({dec[2]})"
            else:
                dec_s = f"Decode::{dk} {{ bits: {dec[1]}, arg: {dec[2]} }}"
            pm_s = "Print::" + self.print_name(pm)
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
                return f"Check::Reg({c[1]}, {self.reg_s(c[2])})"
            if c[0] == "noreg":
                return f"Check::Reg({c[1]}, Reg::NONE)"
            if c[0] == "imm":
                return f"Check::Imm({c[1]}, {c[2]})"
            if c[0] == "immpred":
                s, b, z, nz = c[2]
                return f"Check::ImmFits({c[1]}, ImmRange {{ signed: {str(s).lower()}, bits: {b}, zeros: {z}, nonzero: {str(nz).lower()} }})"
            raise ValueError(c)

        # The instructions each value of an encoding's key bits can be, in
        # the order they're tried: by decoder table, then the most fixed
        # bits first (the more specific encoding).
        for size, label, kb in (
            (4, "BUCKETS32", self.KEY32),
            (2, "BUCKETS16", self.KEY16),
            (8, "BUCKETS64", self.KEY64),
        ):
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
            w(f"static {label}: &[&[u16]] = &[")
            for b in buckets:
                b.sort(key=lambda x: (self.insts[x]["space"], -self.insts[x]["fixed"], x))
                w(f"    &{b},")
            w("];")
            w("")
        w("static ALIASES: &[Alias] = &[")
        for a in self.aliases:
            refs = ", ".join(
                f"Ref {{ op: {o}, print: {('Some(Print::' + self.print_name(p) + ')') if p else 'None'} }}"
                for o, p, _ in a["refs"]
            )
            w(
                f"    Alias {{ inst: {a['inst']}, pred: {a['pred']}, asm: {json.dumps(a['asm'])}, "
                f"checks: &[{', '.join(cond_s(c) for c in a['conds'])}], refs: &[{refs}] }},"
            )
        w("];")
        w("")
        w("static UNCOMPRESS: &[Uncompress] = &[")
        for c in self.compress:
            built = []
            for b in c["built"]:
                if b[0] == "from":
                    built.append(f"Build::From({b[1]})")
                elif b[0] == "reg":
                    built.append(f"Build::Reg({self.reg_s(b[1])})")
                elif b[0] == "imm":
                    built.append(f"Build::Imm({b[1]})")
                else:
                    built.append("Build::Reg(Reg::NONE)")
            w(
                f"    Uncompress {{ from: {c['comp']}, to: {c['full']}, pred: {c['pred']}, "
                f"checks: &[{', '.join(cond_s(x) for x in c['conds'])}], build: &[{', '.join(built)}] }},"
            )
        w("];")
        w("")
        w("pub static TARGET: Target = Target {")
        w("    features: FEATURES,")
        w("    predicates: PREDICATES,")
        w("    classes: CLASSES,")
        w("    kinds: KINDS,")
        w("    instructions: INSTRUCTIONS,")
        w("    aliases: ALIASES,")
        w("    uncompress: UNCOMPRESS,")
        w("    buckets16: BUCKETS16,")
        w(f"    key16: &{self.KEY16},")
        w("    buckets32: BUCKETS32,")
        w(f"    key32: &{self.KEY32},")
        w("    buckets64: BUCKETS64,")
        w(f"    key64: &{self.KEY64},")
        w(f"    spaces: &{self.space_preds},")
        w("};")
        w("")
        self.emit_more(w)
        return "\n".join(L)

    def emit_more(self, w):
        """What else the target writes (RISC-V: its CSRs)."""


class Skip(Exception):
    pass


def main(extractor):
    """Runs an extractor class: `<records .json> <out tables.rs>`."""
    src, out = sys.argv[1], sys.argv[2]
    d = json.load(open(src, encoding="utf-8"))
    E = extractor(d)
    E.run()
    open(out, "w", encoding="utf-8", newline="\n").write(E.emit())
    reasons = {}
    for n, why in E.skipped.items():
        # (By the reason, its instruction's name aside: `X: operand y has
        # no field` and `register decoder D (C)` alike.)
        why = why[len(n) + 2 :] if why.startswith(n + ": ") else why
        why = re.sub(r" \(.*\)$", "", why)
        reasons.setdefault(why, []).append(n)
    print(
        f"{len(E.insts)} instructions ({len(E.skipped)} left out), "
        f"{len(E.aliases)} aliases, {len(E.compress)} compressed forms",
        file=sys.stderr,
    )
    for why, ns in sorted(reasons.items(), key=lambda x: -len(x[1])):
        print(f"  {len(ns)}: {why} ({', '.join(ns[:4])})", file=sys.stderr)
