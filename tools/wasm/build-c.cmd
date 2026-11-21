@echo off
rem Builds testdata\wasm\startup.wasm and startup-reactor.wasm from
rem tools\wasm\startup.c: what runs when a module starts (a command's _start,
rem a reactor's _initialize, wasm-ld's __wasm_call_ctors), with DWARF, by
rem clang and wasm-ld alone; and data.wasm from tools\wasm\data.c: a module's data
rem in linear memory (with DWARF, and wasm-ld's map, data.wasm.map). LLVM_BIN
rem names an LLVM bin directory.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build or release 1>&2 & exit /b 1)
set OUT=%~dp0..\..\testdata\wasm
cd /d "%OUT%" || exit /b 1
set CF=--target=wasm32 -g -O0 -nostdlib -fno-ident
"%LLVM_BIN%\clang.exe" %CF% "%~dp0startup.c" -o startup.wasm || exit /b 1
"%LLVM_BIN%\clang.exe" %CF% -DREACTOR -Wl,--no-entry "%~dp0startup.c" -o startup-reactor.wasm || exit /b 1
rem (Compiled first, so the map names data.o, not a temporary file.)
"%LLVM_BIN%\clang.exe" --target=wasm32 -g -O1 -fno-ident -c "%~dp0data.c" -o data.o || exit /b 1
"%LLVM_BIN%\wasm-ld.exe" --no-entry --export=counter -Map=data.wasm.map data.o -o data.wasm || exit /b 1
del data.o
