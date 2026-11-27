#!/usr/bin/env bash
# Builds the fixture library (a.cpp, b.cpp, c.cpp, which share shared.h's
# inline variables and templates' static data members with dynamic
# initialization) with each compiler, folds it with turbolib under both
# `--pull` modes, links main.cpp against the original and the folded
# library with each linker, and compares the two programs: their output
# (the order every initializer and terminator ran in), `turbolib verify` of
# the libraries, and `turbolib compare-images` of the programs (the
# initializer arrays in order among them). Clang puts each inline
# variable's initialization in the variable's COMDAT group with its
# `.init_array` (or, with -fno-use-init-array, `.ctors`) entry, so those
# units must stay; GCC's are guard variables and data, which fold.
#
#     run.sh TURBOLIB OUT [CONFIG...]
#
# A CONFIG is <compiler>-<optimization>[-ctors], e.g. gcc-O0 or
# clang-O2-ctors (the default: gcc and clang, -O0 and -O2, and clang with
# -fno-use-init-array). LINKERS overrides "bfd gold lld mold". Prints one
# line per check, and exits 1 if any fails.

set -u
TL=$(realpath "$1")
OUT=$2
shift 2
SRC=$(dirname "$(realpath "$0")")
mkdir -p "$OUT"
OUT=$(realpath "$OUT")
configs=("$@")
if [ ${#configs[@]} -eq 0 ]; then
  configs=(gcc-O0 gcc-O2 clang-O0 clang-O2 clang-O0-ctors clang-O2-ctors)
fi
linkers=${LINKERS:-"bfd gold lld mold"}
failed=0
check() { # name, status
  if [ "$2" -eq 0 ]; then echo "ok    $1"; else echo "FAIL  $1"; failed=1; fi
}

for config in "${configs[@]}"; do
  IFS=- read -r cc opt ctors <<<"$config"
  case $cc in
    gcc) cxx=g++ ;;
    clang) cxx=clang++ ;;
    *) echo "unknown compiler $cc"; exit 2 ;;
  esac
  d="$OUT/$config"
  rm -rf "$d"
  mkdir -p "$d"
  flags=(-std=c++17 -g "-$opt" -ffunction-sections -fdata-sections -I"$SRC")
  [ "${ctors:-}" = ctors ] && flags+=(-fno-use-init-array)
  (
    cd "$d" || exit 1
    for n in a b c main; do
      "$cxx" "${flags[@]}" -c "$SRC/$n.cpp" -o "$n.o" || exit 1
    done
    ar qc lib.a c.o a.o b.o && ranlib lib.a
  ) || { check "$config: build" 1; continue; }
  # How many groups hold initializer entries (clang's inline variables).
  grouped=$(for o in a b c; do readelf -gW "$d/$o.o"; done |
    grep -cE '\]   \.(init_array|ctors|fini_array|dtors)')
  for pull in preserve relaxed; do
    p="$config $pull"
    cp "$d/lib.a" "$d/$pull.a"
    "$TL" ar s --fold=identical --pull=$pull --stats "$d/$pull.a" 2>"$d/fold-$pull.txt"
    check "$p: fold" $?
    # No synthesized member may hold an initializer or terminator entry.
    moved=$(readelf -SW "$d/$pull.a" | awk '/^File: .*\(__turbolib\./ { s = 1; next } /^File: / { s = 0 }
      s && /\.(init_array|ctors|fini_array|dtors|preinit_array)/' | wc -l)
    echo "      $p: $(grep -o '[0-9]* folded ([0-9]* copies)' "$d/fold-$pull.txt"); $grouped grouped initializer sections, $moved moved"
    [ "$moved" -eq 0 ]
    check "$p: grouped initializers stay" $?
    "$TL" verify --pull=$pull "$d/lib.a" "$d/$pull.a" >"$d/verify-$pull.txt" 2>&1
    check "$p: verify" $?
    for l in $linkers; do
      ok=0
      for lib in lib $pull; do
        "$cxx" -fuse-ld="$l" "$d/main.o" "$d/$lib.a" -o "$d/$lib-$l" 2>"$d/link-$lib-$l.txt" || ok=1
      done
      check "$p $l: link" $ok
      [ $ok -eq 0 ] || continue
      "$d/lib-$l" >"$d/out-lib-$l.txt"
      "$d/$pull-$l" >"$d/out-$pull-$l.txt"
      cmp -s "$d/out-lib-$l.txt" "$d/out-$pull-$l.txt"
      check "$p $l: output (initialization order)" $?
      "$TL" compare-images "$d/lib-$l" "$d/$pull-$l" >"$d/compare-$pull-$l.txt" 2>&1
      check "$p $l: compare-images" $?
    done
  done
done
exit $failed
