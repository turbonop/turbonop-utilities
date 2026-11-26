#!/bin/sh
# Builds the objects of these sources (for the roundtrip example) into $1,
# with $CLANG (clang by default) and $LIPO (llvm-lipo): C++ and C for x86_64 and arm64 macOS, C
# for i386 macOS (scattered relocations), armv7 iOS, and arm64_32
# watchOS, the assembly files for their targets, and a universal file of
# the C objects. No SDK is needed.
set -e
out=${1:?usage: build.sh <directory>}
cc=${CLANG:-clang}
here=$(dirname "$0")
mkdir -p "$out"
for t in x86_64-apple-macos13 arm64-apple-macos13; do
  "$cc" --target=$t -O2 -g -c "$here/tricky.cpp" -o "$out/tricky-cpp-$t.o"
  "$cc" --target=$t -O0 -g -c "$here/tricky.cpp" -o "$out/tricky-cpp-O0-$t.o"
  "$cc" --target=$t -O2 -g -fcommon -c "$here/tricky.c" -o "$out/tricky-c-$t.o"
done
for t in x86_64-apple-macos13 arm64-apple-macos13 i386-apple-macos10.13 armv7-apple-ios9 arm64_32-apple-watchos5; do
  # The same with a section that moves the rest (remove_section's).
  "$cc" --target=$t -O2 -g -fcommon -DPAD -c "$here/tricky.c" -o "$out/pad-c-$t.o"
done
for t in x86_64-apple-macos13 arm64-apple-macos13; do
  "$cc" --target=$t -O2 -g -DPAD -c "$here/tricky.cpp" -o "$out/pad-cpp-$t.o"
done
for t in i386-apple-macos10.13 armv7-apple-ios9 arm64_32-apple-watchos5; do
  "$cc" --target=$t -O2 -g -fcommon -c "$here/tricky.c" -o "$out/tricky-c-$t.o"
  "$cc" --target=$t -O0 -fcommon -c "$here/tricky.c" -o "$out/tricky-c-O0-$t.o"
done
"$cc" --target=x86_64-apple-macos13 -c "$here/x86_64.s" -o "$out/asm-x86_64.o"
"$cc" --target=arm64-apple-macos13 -c "$here/arm64.s" -o "$out/asm-arm64.o"
"${LIPO:-llvm-lipo}" -create "$out/tricky-c-x86_64-apple-macos13.o" \
  "$out/tricky-c-arm64-apple-macos13.o" -output "$out/tricky-c-universal.o"
