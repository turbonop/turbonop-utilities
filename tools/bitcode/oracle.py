"""Writes the oracles' outputs for the bitcode fixtures in a directory.

    python oracle.py <LLVM bin directory> <testdata/bitcode>

For each bitcode file there: llvm-dis's IR (<file>.ll), llvm-bcanalyzer's
dump and per-block summary (<file>.dump.txt, without the per-record
histogram), and for those with a symbol table, llvm-lto2's view of it
(<file>.symtab.txt). Line endings are \\n; the tools run in the directory,
on bare file names, so no path but the fixtures' own is recorded.
"""

import subprocess
import sys
from pathlib import Path

# The plain bitcode files (the objects and the archive hold theirs).
BITCODE = [
    "kinds.bc",
    "shapes-linux.bc",
    "shapes-windows.bc",
    "kinds-thin.bc",
    "every.bc",
    "embed-darwin.bc",
]
# Files with more than one module: the dump only (llvm-dis writes one).
MULTI = ["multi.bc"]
# Files with an irsymtab (a SYMTAB block).
SYMTAB = ["kinds-thin.bc", "every.bc", "embed-darwin.bc"]


def run(tool: Path, args: list[str], cwd: Path) -> str:
    out = subprocess.run(
        [str(tool), *args], cwd=cwd, capture_output=True, check=True
    )
    return out.stdout.decode("utf-8").replace("\r\n", "\n")


def write(path: Path, text: str) -> None:
    path.write_bytes(text.encode("utf-8"))


def main() -> int:
    bin_dir = Path(sys.argv[1])
    out = Path(sys.argv[2])
    for name in BITCODE:
        subprocess.run(
            [str(bin_dir / "llvm-dis.exe"), name, "-o", name + ".ll"],
            cwd=out,
            check=True,
        )
        ll = out / (name + ".ll")
        write(ll, ll.read_bytes().decode("utf-8").replace("\r\n", "\n"))
    for name in BITCODE + MULTI:
        dump = run(
            bin_dir / "llvm-bcanalyzer.exe",
            ["--dump", "--disable-histogram", name],
            out,
        )
        write(out / (name + ".dump.txt"), dump)
    for name in SYMTAB:
        symtab = run(bin_dir / "llvm-lto2.exe", ["dump-symtab", name], out)
        write(out / (name + ".symtab.txt"), symtab)
    return 0


if __name__ == "__main__":
    sys.exit(main())
