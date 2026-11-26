#!/usr/bin/env bash
# Builds the fixture library (a.cpp, b.cpp, c.cpp, which share shared.h's
# inline functions) with each compiler, DWARF version, and optimization
# level, folds it with turbolib, links main.cpp against the original and
# the folded library with each linker, and compares the two programs:
# their output, `turbolib verify` of the libraries, `turbolib
# compare-images` of the programs (DWARF included), and what gdb shows when
# stopped in folded functions (breakpoints, backtraces, locals, source
# lines), addresses left out.
#
#     run.sh TURBOLIB OUT [CONFIG...]
#
# A CONFIG is <compiler>-<DWARF version>-<optimization>[-split], e.g.
# gcc-5-O0 or clang-4-O2 (the default: every combination of gcc and clang,
# DWARF 4 and 5, -O0 and -O2). LINKERS overrides "bfd gold lld mold".
# Prints one line per check, and exits 1 if any fails.

set -u
TL=$(realpath "$1")
OUT=$2
shift 2
SRC=$(dirname "$(realpath "$0")")
mkdir -p "$OUT"
OUT=$(realpath "$OUT")
configs=("$@")
if [ ${#configs[@]} -eq 0 ]; then
  configs=()
  for c in gcc clang; do for v in 4 5; do for o in O0 O2; do configs+=("$c-$v-$o"); done; done; done
  configs+=(gcc-5-O0-split gcc-5-O2-split clang-5-O0-split clang-5-O2-split)
  configs+=(gcc-4-O0-split gcc-4-O2-split clang-4-O0-split clang-4-O2-split)
fi
linkers=${LINKERS:-"bfd gold lld mold"}
failed=0
check() { # name, status
  if [ "$2" -eq 0 ]; then echo "ok    $1"; else echo "FAIL  $1"; failed=1; fi
}
normalize() {
  sed -E -e 's/0x[0-9a-f]+/ADDR/g' -e 's/process [0-9]+/process N/g' \
    -e 's/Thread [0-9]+/Thread N/g' -e 's/LWP [0-9]+/LWP N/g' "$1"
}


for config in "${configs[@]}"; do
  IFS=- read -r cc v opt split <<<"$config"
  case $cc in
    gcc) cxx=g++ ;;
    clang) cxx=clang++ ;;
    *) echo "unknown compiler $cc"; exit 2 ;;
  esac
  d="$OUT/$config"
  rm -rf "$d"
  mkdir -p "$d"
  flags=(-g "-gdwarf-$v" "-$opt" -ffunction-sections -fdata-sections -I"$SRC")
  [ "${split:-}" = split ] && flags+=(-gsplit-dwarf)
  (
    cd "$d" || exit 1
    for n in a b c main; do
      "$cxx" "${flags[@]}" -c "$SRC/$n.cpp" -o "$n.o" || exit 1
    done
    ar qc lib.a a.o b.o c.o && ranlib lib.a
    cp lib.a folded.a
    "$TL" ar s --fold=identical --stats folded.a 2>fold.txt
  ) || { check "$config: build" 1; continue; }
  grep -o "[0-9]* folded ([0-9]* copies)" "$d/fold.txt" | sed "s/^/      $config: /"
  grep "debug information" "$d/fold.txt" | sed "s/^turbolib: [^:]*: .*synthesized members;/      $config:/"
  "$TL" verify "$d/lib.a" "$d/folded.a" >"$d/verify.txt" 2>&1
  check "$config: verify" $?
  for l in $linkers; do
    ok=0
    for lib in lib folded; do
      "$cxx" -fuse-ld="$l" "$d/main.o" "$d/$lib.a" -o "$d/$lib-$l" 2>"$d/link-$lib-$l.txt" || ok=1
    done
    check "$config $l: link" $ok
    [ $ok -eq 0 ] || continue
    cmp -s <("$d/lib-$l") <("$d/folded-$l")
    check "$config $l: output" $?
    "$TL" compare-images "$d/lib-$l" "$d/folded-$l" >"$d/compare-$l.txt" 2>&1
    check "$config $l: compare-images" $?
    for lib in lib folded; do
      gdb -nx -batch -x "$SRC/gdb.cmd" "$d/$lib-$l" >"$d/gdb-$lib-$l.raw" 2>&1
      normalize "$d/gdb-$lib-$l.raw" >"$d/gdb-$lib-$l.txt"
    done
    if diff "$d/gdb-lib-$l.txt" "$d/gdb-folded-$l.txt" >"$d/gdb-$l.diff"; then
      check "$config $l: gdb" 0
    elif cmp -s <(grep -v '^File ' "$d/gdb-lib-$l.txt" | sort -u) \
      <(grep -v '^File ' "$d/gdb-folded-$l.txt" | sort -u); then
      # GNU ld gives a discarded copy's DWARF the kept copy's addresses, and
      # a split unit's .dwo keeps a discarded copy's DIEs whatever the linker, so
      # the original program describes such a function once per copy
      # (`info functions` lists it under each file); the folded one once.
      echo "ok    $config $l: gdb (but for $(grep -c '^<' "$d/gdb-$l.diff") lines of descriptions of discarded copies)"
    else
      check "$config $l: gdb" 1
    fi
  done
done
exit $failed
