@echo off
rem A Debug, /Z7 build of LLVM, clang, and lld: turbolib's corpus.
if not defined LLVM_SRC set LLVM_SRC=C:\jm\dev\llvm-project
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
cmake -G Ninja -S %LLVM_SRC%\llvm -B %LLVM_SRC%\build-z7-nopch ^
  -DCMAKE_BUILD_TYPE=Debug -DCMAKE_DISABLE_PRECOMPILE_HEADERS=ON ^
  -DCMAKE_POLICY_DEFAULT_CMP0141=NEW ^
  -DCMAKE_MSVC_DEBUG_INFORMATION_FORMAT=Embedded ^
  -DLLVM_ENABLE_PROJECTS="clang;lld" ^
  -DLLVM_TARGETS_TO_BUILD="X86;AArch64" ^
  -DLLVM_OPTIMIZED_TABLEGEN=ON ^
  -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF ^
  -DCLANG_INCLUDE_TESTS=OFF || exit /b 1
