#!/bin/sh
# After build.cmd: how many of the 200 structures, function ids, and
# procedures each PDB holds. Expect 200 structures everywhere; for a-*, 200 ids,
# and 0 procedures for unused* under /OPT:REF.
cd "$(dirname "$0")"
B=${LLVM_BIN:-/c/jm/dev/llvm-project/build-release/bin}
for p in a-REF a-REF-lld a-NOREF a-NOREF-lld p-REF p-REF-lld p-NOREF p-NOREF-lld; do
  s=$($B/llvm-pdbutil dump -types $p.pdb | grep -E 'LF_STRUCTURE .*`S[0-9]+`' | grep -v forward | sed -E 's/.*`(S[0-9]+)`.*/\1/' | sort -u | wc -l)
  i=$($B/llvm-pdbutil dump -ids $p.pdb | grep -c -E 'name = unused[0-9]+')
  g=$($B/llvm-pdbutil dump -symbols $p.pdb | grep 'S_GPROC32' | grep -c 'unused')
  echo "$p: structures $s, unused* function ids $i, unused* procedures $g"
done
