#!/bin/bash
# Builds a configured corpus build at low priority: build.sh gcc|clang|...
set -euo pipefail
name=${1:?usage: build.sh <name>}
CORPUS=${CORPUS:-$HOME/tlcorpus}
NINJA=ninja
[ -x "$CORPUS/tools/ninja" ] && NINJA=$CORPUS/tools/ninja
exec nice -n 19 "$NINJA" -C "$CORPUS/build-$name" "${@:2}"
