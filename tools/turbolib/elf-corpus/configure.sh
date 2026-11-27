#!/bin/bash
# An ELF corpus for turbolib: a Debug (-g) build of LLVM, clang, and lld as
# static libraries, with -ffunction-sections -fdata-sections, by gcc or clang.
#
#   configure.sh gcc|clang|gcc-split|clang-split
#
# LLVM_SRC is an llvm-project checkout (default ~/tlcorpus/src/llvm-project,
# at c74b358f44c3 for the plan's numbers); the build goes to
# $CORPUS/build-<name> (CORPUS default ~/tlcorpus). cmake and ninja are taken
# from $CORPUS/tools if there (setup.sh puts them there), else from PATH.
# The clang builds archive with llvm-ar and llvm-ranlib from the gcc build
# (LLVM_AR overrides), so build gcc first.
set -euo pipefail
name=${1:?usage: configure.sh gcc|clang|gcc-split|clang-split}
CORPUS=${CORPUS:-$HOME/tlcorpus}
LLVM_SRC=${LLVM_SRC:-$CORPUS/src/llvm-project}
CMAKE=cmake; NINJA=ninja
[ -x "$CORPUS/tools/cmake/bin/cmake" ] && CMAKE=$CORPUS/tools/cmake/bin/cmake
[ -x "$CORPUS/tools/ninja" ] && NINJA=$CORPUS/tools/ninja
flags="-ffunction-sections -fdata-sections"
extra=()
case $name in
  gcc|gcc-split) cc=gcc; cxx=g++ ;;
  clang|clang-split)
    cc=clang; cxx=clang++
    bin=${LLVM_AR_DIR:-$CORPUS/build-gcc/bin}
    extra+=(-DCMAKE_AR="$bin/llvm-ar" -DCMAKE_RANLIB="$bin/llvm-ranlib")
    ;;
  *) echo "unknown build $name" >&2; exit 2 ;;
esac
case $name in
  *-split) flags="$flags -gsplit-dwarf" ;;
esac
exec "$CMAKE" -G Ninja -S "$LLVM_SRC/llvm" -B "$CORPUS/build-$name" \
  -DCMAKE_MAKE_PROGRAM="$NINJA" \
  -DCMAKE_C_COMPILER=$cc -DCMAKE_CXX_COMPILER=$cxx \
  -DCMAKE_BUILD_TYPE=Debug \
  -DCMAKE_C_FLAGS="$flags" -DCMAKE_CXX_FLAGS="$flags" \
  -DLLVM_ENABLE_PROJECTS="clang;lld" \
  -DLLVM_TARGETS_TO_BUILD="X86;AArch64" \
  -DLLVM_OPTIMIZED_TABLEGEN=ON \
  -DLLVM_USE_LINKER=lld \
  -DLLVM_PARALLEL_LINK_JOBS=2 \
  -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF \
  -DCLANG_INCLUDE_TESTS=OFF \
  "${extra[@]}"
