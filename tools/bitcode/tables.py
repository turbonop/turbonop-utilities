"""Generates crates/formats/bitcode/src/tables.rs: LLVM bitcode's block
IDs and record codes (every enum of Bitcode/LLVMBitCodes.h and
Bitstream/BitCodeEnums.h), the names llvm-bcanalyzer gives blocks and
records (Bitcode/Reader/BitcodeAnalyzer.cpp), and attribute kinds'
spellings (the reader's getAttrFromCode and IR/Attributes.td), from
LLVM's sources (Apache License 2.0 with LLVM Exceptions; see
crates/formats/bitcode/LICENSE-LLVM).

    python -I tools/bitcode/tables.py <llvm-project> > crates/formats/bitcode/src/tables.rs
"""

import os
import re
import sys

# Attribute kinds the reader upgrades to others (getAttrFromCode doesn't
# map them): their spelling when they were attributes.
LEGACY_ATTRIBUTES = {
    "ATTR_KIND_NO_CAPTURE": "nocapture",
    "ATTR_KIND_READ_NONE": "readnone",
    "ATTR_KIND_READ_ONLY": "readonly",
    "ATTR_KIND_ARGMEMONLY": "argmemonly",
    "ATTR_KIND_INACCESSIBLEMEM_ONLY": "inaccessiblememonly",
    "ATTR_KIND_INACCESSIBLEMEM_OR_ARGMEMONLY": "inaccessiblemem_or_argmemonly",
    "ATTR_KIND_WRITEONLY": "writeonly",
}


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return re.sub(r"//[^\n]*", "", text)


def enums(text):
    """Every `enum Name { A = 1, B, ... };` (or unnamed): (name, [(A, value)])."""
    out = []
    for m in re.finditer(r"\benum\s+(\w+)?\s*(?::\s*\w+\s*)?\{(.*?)\}", text, re.S):
        name = m.group(1) or ""
        value = -1
        entries = []
        for item in m.group(2).split(","):
            item = item.strip()
            if not item:
                continue
            if "=" in item:
                k, v = (s.strip() for s in item.split("=", 1))
                v = v.replace(" ", "")
                if re.fullmatch(r"\d+|0x[0-9A-Fa-f]+", v):
                    value = int(v, 0)
                elif m2 := re.fullmatch(r"(\d+)\*4", v):
                    value = int(m2.group(1)) * 4
                elif m2 := re.fullmatch(r"1<<(\d+)", v):
                    value = 1 << int(m2.group(1))
                elif m2 := re.fullmatch(r"\((\d+)<<(\d+)\)", v):
                    value = int(m2.group(1)) << int(m2.group(2))
                else:
                    known = {k2: v2 for _, es in out for k2, v2 in es}
                    known.update(dict(entries))
                    if m2 := re.fullmatch(r"(\w+)\+(\d+)", v):
                        value = known[m2.group(1)] + int(m2.group(2))
                    elif v in known:
                        value = known[v]
                    elif v.startswith("bitc::") and v[6:] in known:
                        value = known[v[6:]]
                    else:
                        raise SystemExit(f"can't evaluate {k} = {v}")
            else:
                k = item
                value += 1
            entries.append((k, value))
        out.append((name, entries))
    return out


def switch_cases(body):
    """`case X: return "Y";` and STRINGIFY_CODE(P, C) pairs of a switch body."""
    out = []
    pending = []
    for m in re.finditer(
        r"case\s+bitc::(\w+)\s*:|STRINGIFY_CODE\((\w+),\s*(\w+)\)|return\s+\"(\w+)\"\s*;",
        body,
    ):
        if m.group(1):
            pending.append(m.group(1))
        elif m.group(2):
            out.append((f"{m.group(2)}_{m.group(3)}", m.group(3)))
        elif pending:
            for p in pending:
                out.append((p, m.group(4)))
            pending = []
    return out


def function_body(text, signature):
    start = text.index(signature)
    i = text.index("{", start)
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[i : j + 1]
    raise SystemExit(f"no body for {signature}")


def main():
    root = sys.argv[1]
    llvm = os.path.join(root, "llvm")
    codes_h = strip_comments(
        open(os.path.join(llvm, "include/llvm/Bitcode/LLVMBitCodes.h"), encoding="utf-8").read()
    )
    enums_h = strip_comments(
        open(os.path.join(llvm, "include/llvm/Bitstream/BitCodeEnums.h"), encoding="utf-8").read()
    )
    analyzer = strip_comments(
        open(os.path.join(llvm, "lib/Bitcode/Reader/BitcodeAnalyzer.cpp"), encoding="utf-8").read()
    )
    reader = strip_comments(
        open(os.path.join(llvm, "lib/Bitcode/Reader/BitcodeReader.cpp"), encoding="utf-8").read()
    )
    attributes_td = open(os.path.join(llvm, "include/llvm/IR/Attributes.td"), encoding="utf-8").read()

    groups = enums(enums_h)
    # Block IDs continue from FIRST_APPLICATION_BLOCKID.
    base = {k: v for _, es in groups for k, v in es}
    codes_h = codes_h.replace("FIRST_APPLICATION_BLOCKID", str(base["FIRST_APPLICATION_BLOCKID"]))
    groups += enums(codes_h)
    value = {k: v for _, es in groups for k, v in es}

    out = []
    w = out.append
    w("//! LLVM bitcode's block IDs and record codes, the names llvm-bcanalyzer")
    w("//! gives blocks and records, and attribute kinds' spellings.")
    w("//!")
    w("//! Generated by tools/bitcode/tables.py from LLVM's Bitcode/LLVMBitCodes.h,")
    w("//! Bitstream/BitCodeEnums.h, Bitcode/Reader/BitcodeAnalyzer.cpp,")
    w("//! Bitcode/Reader/BitcodeReader.cpp, and IR/Attributes.td (Apache License")
    w("//! 2.0 with LLVM Exceptions: see LICENSE-LLVM). Do not edit.")
    w("")
    w("#![allow(missing_docs, non_upper_case_globals, clippy::match_same_arms, clippy::too_many_lines)]")
    w("")
    seen = set()
    for name, entries in groups:
        if not entries:
            continue
        w(f"// enum {name}" if name else "// enum")
        for k, v in entries:
            if k in seen:
                continue
            seen.add(k)
            w(f"pub const {k}: u32 = {v};")
        w("")

    # Block names: GetBlockName's switch (and BLOCKINFO_BLOCK).
    body = function_body(analyzer, "GetBlockName(unsigned BlockID")
    w("/// The name llvm-bcanalyzer gives an LLVM IR block (without a BLOCKINFO")
    w("/// name for it).")
    w("pub fn block_name(id: u32) -> Option<&'static str> {")
    w("    Some(match id {")
    w('        0 => "BLOCKINFO_BLOCK",')
    for k, n in switch_cases(body.split("switch (BlockID)", 1)[1]):
        w(f'        {k} => "{n}",')
    w("        _ => return None,")
    w("    })")
    w("}")
    w("")

    # Record names: GetCodeName's nested switches, by block.
    body = function_body(analyzer, "GetCodeName(unsigned CodeID")
    w("/// The name llvm-bcanalyzer gives a record of an LLVM IR block (or of")
    w("/// BLOCKINFO), without a BLOCKINFO name for it.")
    w("pub fn record_name(block: u32, code: u32) -> Option<&'static str> {")
    w("    Some(match (block, code) {")
    # BLOCKINFO's own records.
    w('        (0, BLOCKINFO_CODE_SETBID) => "SETBID",')
    w('        (0, BLOCKINFO_CODE_BLOCKNAME) => "BLOCKNAME",')
    w('        (0, BLOCKINFO_CODE_SETRECORDNAME) => "SETRECORDNAME",')
    main_switch = body.split("switch (BlockID) {", 1)[1]
    # Split into `case bitc::BLOCK:` groups, each with its inner switch.
    parts = re.split(r"\n  case\s+bitc::(\w+)\s*:", "\n" + main_switch)
    pending_blocks = []
    for i in range(1, len(parts), 2):
        block = parts[i]
        inner = parts[i + 1]
        pending_blocks.append(block)
        if "switch (CodeID)" not in inner:
            continue
        for k, n in switch_cases(inner.split("switch (CodeID)", 1)[1]):
            for b in pending_blocks:
                w(f'        ({b}, {k}) => "{n}",')
        pending_blocks = []
    w("        _ => return None,")
    w("    })")
    w("}")
    w("")

    # Attribute spellings.
    body = function_body(reader, "static Attribute::AttrKind getAttrFromCode(uint64_t Code)")
    td = {
        m.group(1): m.group(2)
        for m in re.finditer(r"^def (\w+)\s*:\s*\w*Attr<\"([^\"]+)\"", attributes_td, re.M)
    }
    names = {}
    for code, kind in re.findall(r"case\s+bitc::(\w+)\s*:\s*return\s+Attribute::(\w+)\s*;", body):
        if kind in td:
            names[code] = td[kind]
    for code, name in LEGACY_ATTRIBUTES.items():
        names.setdefault(code, name)
    w("/// The IR spelling of an attribute kind (`ATTR_KIND_*`).")
    w("pub fn attribute_name(kind: u32) -> Option<&'static str> {")
    w("    Some(match kind {")
    for k, v in sorted(names.items(), key=lambda kv: value[kv[0]]):
        w(f'        {k} => "{v}",')
    w("        _ => return None,")
    w("    })")
    w("}")
    sys.stdout.buffer.write(("\n".join(out) + "\n").encode("utf-8"))


if __name__ == "__main__":
    main()
