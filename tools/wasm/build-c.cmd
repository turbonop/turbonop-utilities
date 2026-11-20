@echo off
rem Builds testdata\wasm\startup.wasm and startup-reactor.wasm from
rem tools\wasm\startup.c: what runs when a module starts (a command's _start,
rem a reactor's _initialize, wasm-ld's __wasm_call_ctors), with DWARF, by
rem clang and wasm-ld alone. LLVM_BIN names an LLVM bin directory.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build or release 1>&2 & exit /b 1)
set OUT=%~dp0..\..\testdata\wasm
cd /d "%OUT%" || exit /b 1
set CF=--target=wasm32 -g -O0 -nostdlib -fno-ident
"%LLVM_BIN%\clang.exe" %CF% "%~dp0startup.c" -o startup.wasm || exit /b 1
"%LLVM_BIN%\clang.exe" %CF% -DREACTOR -Wl,--no-entry "%~dp0startup.c" -o startup-reactor.wasm || exit /b 1
