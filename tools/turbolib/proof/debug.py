"""Debugs a program with cdb at a function: a breakpoint, the stack, the
locals, and the source line, so that a folded build's debugging can be
compared with lib.exe's.

    python debug.py <program> <function> [--cdb CDB] -- <arguments>...

cdb (the WinDbg package's command-line debugger) is run on <program> with
<arguments>, breaks at the first call of <function> (a `bm` pattern, such as
`clang!llvm::StringRef::find`), and prints the stack (`k`), the locals with
their types and values (`dv /t /v`), the source line, and the caller's
frame and locals. Its output is printed with the addresses, which
differ between two links, replaced by `<addr>`, so that two sessions can be
compared with `diff`.
"""
import argparse
import os
import re
import subprocess
import sys

CDB = os.path.join(os.environ.get("ProgramFiles", r"C:\Program Files"), "WindowsApps",
                   "Microsoft.WinDbg_1.2611.4001.0_x64__8wekyb3d8bbwe", "amd64", "cdb.exe")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("program")
    ap.add_argument("function")
    ap.add_argument("--cdb", default=CDB)
    ap.add_argument("--raw", action="store_true", help="don't hide addresses")
    ap.add_argument("args", nargs="*")
    a = ap.parse_args()
    sym = os.path.dirname(os.path.abspath(a.program))
    commands = ";".join([
        ".lines -e",
        f"bm {a.function}",
        "g",
        ".echo ==STACK==",
        "kn 12",
        ".echo ==LOCALS==",
        "dv /t /v",
        ".echo ==LINE==",
        ".frame 0",
        ".echo ==UP==",
        ".frame 1",
        "dv /t",
        ".echo ==END==",
        "q"])
    args = [a.cdb, "-lines", "-y", sym, "-c", commands, os.path.abspath(a.program)] + a.args
    p = subprocess.run(args, capture_output=True, text=True, errors="replace", timeout=600)
    out = p.stdout
    start = out.find("==STACK==")
    end = out.find("==END==")
    text = out[start:end + 7] if 0 <= start < end else out
    if not a.raw:
        text = re.sub(r"\b(0x)?[0-9a-f]{8}`[0-9a-f]{8}\b", "<addr>", text)
        text = re.sub(r"\b0x[0-9a-f]{6,16}\b", "<addr>", text)
        text = re.sub(r"\+0x[0-9a-f]+\b", "+<off>", text)
        text = re.sub(r"\b[0-9a-f]{16}\b", "<addr>", text)
    print(text)
    if start < 0 or "==END==" not in out:
        print("debug.py: the session didn't reach the breakpoint", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
