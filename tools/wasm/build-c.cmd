@echo off
rem Builds testdata\wasm\startup.wasm and startup-reactor.wasm from
rem tools\wasm\startup.c: what runs when a module starts (a command's _start,
rem a reactor's _initialize, wasm-ld's __wasm_call_ctors), with DWARF, by
rem clang and wasm-ld alone; and data.wasm from tools\wasm\data.c: a
rem module's data in linear memory (with DWARF, and wasm-ld's map,
rem data.wasm.map), and data-shared.wasm and data-pic.wasm, the same with
rem shared memory and as a position-independent module. LLVM_BIN names an
rem LLVM bin directory.
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
rem The same with shared memory (passive segments, which
rem __wasm_init_memory copies, and .bss it zeroes), and as a
rem position-independent module (its data from its __memory_base).
"%LLVM_BIN%\clang.exe" --target=wasm32 -g -O1 -fno-ident -matomics -mbulk-memory -c "%~dp0data.c" -o data-shared.o || exit /b 1
"%LLVM_BIN%\wasm-ld.exe" --no-entry --export=counter --shared-memory --max-memory=131072 data-shared.o -o data-shared.wasm || exit /b 1
"%LLVM_BIN%\clang.exe" --target=wasm32 -g -O1 -fno-ident -fPIC -c "%~dp0data.c" -o data-pic.o || exit /b 1
"%LLVM_BIN%\wasm-ld.exe" --experimental-pic -shared --export=counter data-pic.o -o data-pic.wasm || exit /b 1
del data-shared.o data-pic.o
