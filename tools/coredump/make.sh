#!/bin/sh
# Builds crash.c, runs it until it crashes, and keeps its core file and
# gdb's view of it (the oracle) in testdata/coredump.
#
# Run in Linux (WSL), from anywhere: sh tools/coredump/make.sh. The program
# runs from /tmp/da with an empty environment, so the core file holds no
# paths or variables of the machine's but its system libraries'.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../../testdata/coredump"
mkdir -p "$out" /tmp/da
cp "$here/crash.c" /tmp/da/crash.c
cd /tmp/da
rm -f core core.*
gcc -O1 -g -fno-omit-frame-pointer -pthread -o crash-x86_64 crash.c
ulimit -c unlimited
env -i ./crash-x86_64 || true
core=$(ls core* | head -n 1)
cp crash-x86_64 "$out/crash-x86_64"
cp "$core" "$out/crash-x86_64.core"
gdb -nx -batch \
    -ex 'info threads' \
    -ex 'thread apply all bt' \
    -ex 'info registers' \
    -ex 'info proc mappings' \
    ./crash-x86_64 "$core" > "$out/crash-x86_64.gdb.txt" 2>&1
readelf -n "$core" > "$out/crash-x86_64.notes.txt"
ls -l "$out"
