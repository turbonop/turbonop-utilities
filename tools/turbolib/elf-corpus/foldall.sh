#!/usr/bin/env bash
# Folds every static library of a corpus build with turbolib, one after
# another (turbolib uses every core on each), into OUT under the build's
# names, and gathers each run's --stats line into OUT/fold-stats.txt.
#
#     foldall.sh TURBOLIB BUILD/lib OUT [turbolib options...]
#
# The options default to --fold=identical. Run it at low priority (nice).

set -eu
TL=$(realpath "$1")
LIB=$(realpath "$2")
OUT=$3
shift 3
opts=("$@")
[ ${#opts[@]} -gt 0 ] || opts=(--fold=identical)
mkdir -p "$OUT"
: >"$OUT/fold-stats.txt"
start=$(date +%s.%N)
for a in "$LIB"/*.a; do
  n=$(basename "$a")
  cp "$a" "$OUT/$n"
  "$TL" ar s "${opts[@]}" --stats "$OUT/$n" 2>>"$OUT/fold-stats.txt"
done
end=$(date +%s.%N)
echo "folded $(ls "$LIB"/*.a | wc -l) libraries in $(echo "$end - $start" | bc) s" | tee -a "$OUT/fold-stats.txt"
