@echo off
if not defined LLVM_SRC set LLVM_SRC=C:\jm\dev\llvm-project
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
ninja -C %LLVM_SRC%\build-z7 llvm-libraries clang-libraries lldCOFF
