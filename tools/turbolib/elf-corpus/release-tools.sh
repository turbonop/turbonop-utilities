#!/bin/bash
# A Release build of llvm-ar (and llvm-ranlib), for timing turbolib against
# it: the corpus builds' own llvm-ar is a Debug build (-O0), far slower than
# the one a user runs.
#
#   release-tools.sh
#
# Builds $CORPUS/build-release (CORPUS default ~/tlcorpus) from LLVM_SRC
# (default $CORPUS/src/llvm-project) with gcc, at low priority, and links
# ar, ranlib, llvm-ar, and llvm-ranlib to it in $CORPUS/llvm-release, a
# directory for relib.py --time --against.
set -euo pipefail
CORPUS=${CORPUS:-$HOME/tlcorpus}
LLVM_SRC=${LLVM_SRC:-$CORPUS/src/llvm-project}
CMAKE=cmake; NINJA=ninja
[ -x "$CORPUS/tools/cmake/bin/cmake" ] && CMAKE=$CORPUS/tools/cmake/bin/cmake
[ -x "$CORPUS/tools/ninja" ] && NINJA=$CORPUS/tools/ninja
"$CMAKE" -G Ninja -S "$LLVM_SRC/llvm" -B "$CORPUS/build-release" \
  -DCMAKE_MAKE_PROGRAM="$NINJA" \
  -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++ \
  -DCMAKE_BUILD_TYPE=Release \
  -DLLVM_TARGETS_TO_BUILD="X86;AArch64" \
  -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF
nice -n 19 "$NINJA" -C "$CORPUS/build-release" llvm-ar
mkdir -p "$CORPUS/llvm-release"
for n in ar llvm-ar; do ln -sf "$CORPUS/build-release/bin/llvm-ar" "$CORPUS/llvm-release/$n"; done
for n in ranlib llvm-ranlib; do ln -sf "$CORPUS/build-release/bin/llvm-ranlib" "$CORPUS/llvm-release/$n"; done
"$CORPUS/llvm-release/llvm-ar" --version
