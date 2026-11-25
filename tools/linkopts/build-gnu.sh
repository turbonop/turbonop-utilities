#!/bin/sh
# Builds the link-option test inputs of GCC and the GNU-compatible linkers
# into testdata/linkopts, as build.cmd does with clang and ld.lld: static
# x86-64 executables, with DWARF, for what hexcavator recommend judges a link
# did from what it left. Run on Linux, from the repository, with gcc, GNU ld
# (ld.bfd), gold (ld.gold), and mold:
#
#   gnu-<linker>-funcs-<link>
#                 funcs.c (-ffunction-sections -fdata-sections; -fno-ipa-icf:
#                 the compiler folds nothing), linked by <linker> (bfd,
#                 gold, mold) plain, -gc, and (gold, mold: GNU ld has no
#                 ICF) -icf and -gc-icf.
#   gnu-<linker>-eh, gnu-<linker>-eh-icf
#                 eh.cpp twice (N=1, N=2) and eh-main.cpp, with exceptions,
#                 linked with --gc-sections (and --icf=all).
#   gnu-<linker>-dup, gnu-<linker>-dup-icf
#                 dup.c three times, other.c, and dup-main.c, linked with
#                 --gc-sections (and --icf=all).
#
# Each with its linker's map beside it (<name>.map: -Map), for hexcavator
# linkerlog and size -map.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out="$root/testdata/linkopts"
work="$root/target/fixtures/linkopts-gnu"
rm -rf "$work"
mkdir -p "$work" "$out"
cd "$work"

# (No path of this machine's recorded: sources as tools/linkopts/..., the
# build directory as `.`.)
CC="gcc -O2 -g -ffreestanding -fno-stack-protector -fno-ident -fno-pie -ffile-prefix-map=$root/= -fdebug-prefix-map=$work=."
CXX="g++ -O2 -g -ffreestanding -fno-stack-protector -fno-ident -fno-pie -ffile-prefix-map=$root/= -fdebug-prefix-map=$work=."
SECTIONS="-ffunction-sections -fdata-sections"
NOICF="-fno-ipa-icf"

$CC $SECTIONS $NOICF -fno-asynchronous-unwind-tables -c "$here/funcs.c" -o funcs.o
$CXX $SECTIONS $NOICF -DN=1 -c "$here/eh.cpp" -o eh1.o
$CXX $SECTIONS $NOICF -DN=2 -c "$here/eh.cpp" -o eh2.o
$CXX $SECTIONS $NOICF -c "$here/eh-main.cpp" -o eh-main.o
for u in use_a use_b use_c; do
  $CC $SECTIONS $NOICF -fno-asynchronous-unwind-tables -DUSE=$u -c "$here/dup.c" -o $u.o
done
$CC $SECTIONS $NOICF -fno-asynchronous-unwind-tables -c "$here/other.c" -o other.o
$CC $SECTIONS $NOICF -fno-asynchronous-unwind-tables -c "$here/dup-main.c" -o dup-main.o

# link <linker> <name> <options...> -- <objects...>: the executable and its
# map, into testdata/linkopts.
link() {
  linker=$1
  name=$2
  shift 2
  case $linker in
    bfd) ld=ld.bfd ;;
    gold) ld=ld.gold ;;
    mold) ld=mold ;;
  esac
  $ld -static --build-id=sha1 -e _start -Map="$name.map" "$@" -o "$name"
  cp "$name" "$out/$name"
  # (The map's paths: the build directory's, as `.`.)
  sed "s#$work/##g; s#$work#.#g" "$name.map" > "$out/$name.map"
}

for l in bfd gold mold; do
  link $l gnu-$l-funcs-plain funcs.o
  link $l gnu-$l-funcs-gc --gc-sections funcs.o
  # (eh.cpp's personality and unwinder aren't linked: nothing runs these.)
  link $l gnu-$l-eh --gc-sections --unresolved-symbols=ignore-all eh1.o eh2.o eh-main.o
  link $l gnu-$l-dup --gc-sections use_a.o use_b.o use_c.o other.o dup-main.o
  if [ $l != bfd ]; then
    link $l gnu-$l-funcs-icf --icf=all funcs.o
    link $l gnu-$l-funcs-gc-icf --gc-sections --icf=all funcs.o
    link $l gnu-$l-eh-icf --gc-sections --icf=all --unresolved-symbols=ignore-all eh1.o eh2.o eh-main.o
    link $l gnu-$l-dup-icf --gc-sections --icf=all use_a.o use_b.o use_c.o other.o dup-main.o
  fi
done
echo "built into $out"
