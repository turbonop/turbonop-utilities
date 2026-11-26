#!/bin/bash
# Fetches cmake and ninja (into $CORPUS/tools) and llvm-project at the
# corpus commit (into $CORPUS/src), without root. CORPUS: ~/tlcorpus.
set -euo pipefail
CORPUS=${CORPUS:-$HOME/tlcorpus}
mkdir -p "$CORPUS/tools" "$CORPUS/src"
cd "$CORPUS/tools"
if [ ! -x cmake/bin/cmake ]; then
  curl -fsSL -o cmake.tgz https://github.com/Kitware/CMake/releases/download/v3.31.6/cmake-3.31.6-linux-x86_64.tar.gz
  mkdir -p cmake && tar -xzf cmake.tgz -C cmake --strip-components=1 && rm cmake.tgz
fi
if [ ! -x ninja ]; then
  curl -fsSL -o ninja.zip https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-linux.zip
  python3 -c "import zipfile; zipfile.ZipFile('ninja.zip').extractall('.')" && chmod +x ninja && rm ninja.zip
fi
cd "$CORPUS/src"
if [ ! -d llvm-project/.git ]; then
  git init -q llvm-project
  cd llvm-project
  git remote add origin https://github.com/llvm/llvm-project.git
  git fetch -q --depth 1 origin c74b358f44c31e3615fe942556a41be8c9a8d877
  git checkout -q FETCH_HEAD
fi
echo SETUP_DONE
