@echo off
rem Builds testdata\mingw: a PE image with DWARF, as clang --target=*-windows-gnu
rem (MinGW's toolchain) links one with lld, freestanding (no CRT): program.exe;
rem the same stripped, its DWARF in program.debug, which .gnu_debuglink names
rem (program-stripped.exe); and an ARM64 build (program-arm64.exe).
rem LLVM_BIN names an LLVM bin directory.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build or release 1>&2 & exit /b 1)
set OUT=%~dp0..\..\testdata\mingw
if not exist "%OUT%" mkdir "%OUT%"
cd /d "%OUT%" || exit /b 1
set CF=-g -O1 -ffreestanding -nostdlib -fno-ident -fuse-ld=lld -ffunction-sections -fdata-sections -Wl,-e,start -Wl,--subsystem,console -Wl,--no-insert-timestamp
"%LLVM_BIN%\clang.exe" --target=x86_64-w64-windows-gnu %CF% "%~dp0program.c" -o program.exe || exit /b 1
"%LLVM_BIN%\clang.exe" --target=aarch64-w64-windows-gnu %CF% "%~dp0program.c" -o program-arm64.exe || exit /b 1
"%LLVM_BIN%\llvm-objcopy.exe" --only-keep-debug program.exe program.debug || exit /b 1
"%LLVM_BIN%\llvm-objcopy.exe" --strip-debug --add-gnu-debuglink=program.debug program.exe program-stripped.exe || exit /b 1
