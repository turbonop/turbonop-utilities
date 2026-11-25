"""Archives every static library of a Ninja build again with another tool,
and compares each with the library the build made.

    python relib.py <build dir> <tool> [--flavor lib|llvm-lib] [--jobs N]
                    [--keep DIR] [--only NAME...]

<tool> is run as the build ran its archiver: the same arguments (the rule's
command after the archiver's path, with $LINK_FLAGS, /out:, and $in or the
response file), in the build directory, with /out: pointing at a scratch
copy. Each result is compared with the build's library, byte for byte,
except for the header date fields (lib.exe stamps the time of archiving
unless /Brepro is given).

A run with lib.exe itself as <tool> checks this script: every library must
come out the same.

Run it from any directory; it reads <build dir>/build.ninja and
<build dir>/CMakeFiles/rules.ninja.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor


def ninja_lines(path):
    """Logical lines: `$` at the end of a line continues it."""
    with open(path, encoding="utf-8", errors="replace") as f:
        buf = ""
        for raw in f:
            line = raw.rstrip("\r\n")
            if line.endswith("$") and not line.endswith("$$"):
                buf += line[:-1]
                continue
            yield buf + line
            buf = ""
        if buf:
            yield buf


def unescape(s):
    return re.sub(r"\$([ :$])", r"\1", s)


def split_paths(s):
    """Ninja path lists: spaces separate, `$ ` is a literal space."""
    out, cur, i = [], "", 0
    while i < len(s):
        c = s[i]
        if c == "$" and i + 1 < len(s):
            cur += s[i + 1]
            i += 2
            continue
        if c == " ":
            if cur:
                out.append(cur)
            cur = ""
        else:
            cur += c
        i += 1
    if cur:
        out.append(cur)
    return out


def read_rules(path):
    rules, name = {}, None
    for line in ninja_lines(path):
        m = re.match(r"rule (\S+)", line)
        if m:
            name = m.group(1)
            rules[name] = {}
            continue
        m = re.match(r"\s+(\w+) = (.*)", line)
        if m and name:
            rules[name][m.group(1)] = m.group(2)
        elif not line.startswith(" "):
            name = None
    return rules


def read_libraries(build_dir, rules):
    """Each static library's build statement: outputs, inputs, variables."""
    libs, cur = [], None
    for line in ninja_lines(os.path.join(build_dir, "build.ninja")):
        m = re.match(r"build (.+?): (\S+)(.*)", line)
        if m:
            cur = None
            rule = m.group(2)
            if "STATIC_LIBRARY_LINKER" in rule and rule in rules:
                outputs = split_paths(m.group(1))
                explicit = m.group(3).split(" || ")[0].split(" | ")[0]
                cur = {"out": outputs[0], "rule": rule, "in": split_paths(explicit), "vars": {}}
                libs.append(cur)
            continue
        m = re.match(r"\s+(\w+) = (.*)", line)
        if m and cur is not None:
            cur["vars"][m.group(1)] = m.group(2)
        elif not line.startswith(" "):
            cur = None
    return libs


def expand(template, values):
    return re.sub(r"\$(\w+)", lambda m: values.get(m.group(1), ""), template)


def archiver_args(lib, rules, out_path, rsp_path):
    """The archiver's arguments for `lib`, with /out: at `out_path`."""
    rule = rules[lib["rule"]]
    values = dict(lib["vars"])
    values["in"] = " ".join(lib["in"])
    values["in_newline"] = "\n".join(lib["in"])
    values["TARGET_FILE"] = out_path
    values["RSP_FILE"] = rsp_path
    command = unescape(rule["command"])
    # cmd.exe /C "$PRE_LINK && <archiver> <args> && $POST_BUILD"
    m = re.search(r'&& (\S+?lib\.exe|\S+?llvm-lib\.exe) (.*?) && \$POST_BUILD', command, re.I)
    if not m:
        raise ValueError(f"{lib['out']}: unrecognized archiver command: {command}")
    args = expand(m.group(2), values)
    rsp = None
    if "rspfile_content" in rule:
        rsp = expand(rule["rspfile_content"], values)
    return args.split(), rsp


def headers(data):
    """The offsets of every member header's date field in an archive."""
    if not data.startswith(b"!<arch>\n"):
        return []
    out, at = [], 8
    while at + 60 <= len(data):
        size = int(data[at + 48:at + 58].strip() or b"0")
        out.append(at + 16)
        at += 60 + size + (size & 1)
    return out


def same(a, b):
    """Equal except for member headers' date fields."""
    if len(a) != len(b):
        return False, f"sizes {len(a)} and {len(b)}"
    da, db = headers(a), headers(b)
    if da != db:
        return False, "member layout differs"
    ma, mb = bytearray(a), bytearray(b)
    for at in da:
        ma[at:at + 12] = b" " * 12
        mb[at:at + 12] = b" " * 12
    if ma == mb:
        return True, ""
    first = next(i for i in range(len(ma)) if ma[i] != mb[i])
    return False, f"first difference at 0x{first:X}"


def run_one(build_dir, tool, lib, rules, scratch):
    name = lib["out"]
    out_path = os.path.join(scratch, name.replace("\\", "_").replace("/", "_"))
    rsp_path = out_path + ".rsp"
    args, rsp = archiver_args(lib, rules, out_path, rsp_path)
    if rsp is not None:
        with open(rsp_path, "w", encoding="utf-8") as f:
            f.write(rsp)
    p = subprocess.run([tool] + args, cwd=build_dir, capture_output=True)
    if p.returncode != 0:
        return name, False, f"exit {p.returncode}: {p.stdout.decode(errors='replace').strip()}"
    with open(os.path.join(build_dir, name), "rb") as f:
        original = f.read()
    with open(out_path, "rb") as f:
        made = f.read()
    ok, why = same(original, made)
    if ok:
        os.remove(out_path)
        if rsp is not None:
            os.remove(rsp_path)
    return name, ok, why


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("build_dir")
    ap.add_argument("tool")
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    ap.add_argument("--keep", help="where to leave results that differ")
    ap.add_argument("--only", nargs="*", help="library paths to run (as build.ninja names them)")
    a = ap.parse_args()
    rules = read_rules(os.path.join(a.build_dir, "CMakeFiles", "rules.ninja"))
    libs = read_libraries(a.build_dir, rules)
    if a.only:
        libs = [l for l in libs if l["out"] in a.only]
    # A build of some targets leaves the other libraries unbuilt.
    built = [l for l in libs if os.path.exists(os.path.join(a.build_dir, l["out"]))]
    print(f"{len(built)} of {len(libs)} libraries were built")
    libs = built
    scratch = a.keep or tempfile.mkdtemp(prefix="relib-")
    os.makedirs(scratch, exist_ok=True)
    with ThreadPoolExecutor(a.jobs) as pool:
        results = list(pool.map(lambda l: run_one(a.build_dir, a.tool, l, rules, scratch), libs))
    bad = [(n, why) for n, ok, why in results if not ok]
    for n, why in bad:
        print(f"DIFFERS {n}: {why}")
    print(f"{len(results) - len(bad)} of {len(results)} libraries the same")
    if not a.keep and not bad:
        shutil.rmtree(scratch, ignore_errors=True)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
