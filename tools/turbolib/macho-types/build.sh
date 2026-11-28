#!/bin/sh
# Builds the objects of turbolib's --debug-types tests (turbonop-fold's
# macho tests) into testdata/turbolib/macho-types: a.cpp and b.cpp, each
# with a copy of types.h's inline functions (weak definitions: they fold
# into one synthesized member), for arm64 macOS at -O0 with DWARF. a's and
# b's own code names Point (a parameter, a local); only the inline
# functions name Hidden. No SDK is needed.
#
#   sh tools/turbolib/macho-types/build.sh
set -e
here=$(dirname "$0")
out="$here/../../../testdata/turbolib/macho-types"
cc=${CLANG:-clang++}
mkdir -p "$out"
for s in a b; do
  "$cc" --target=arm64-apple-macos13 -O0 -g -c "$here/$s.cpp" -o "$out/$s-arm64.o"
done
