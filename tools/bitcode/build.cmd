@echo off
rem Builds the LLVM bitcode fixtures in testdata\bitcode from kinds.c,
rem shapes.cpp, embed.c, and every.ll, with clang and the LLVM tools in
rem %LLVM_BIN% (by default an LLVM build at
rem C:\jm\dev\llvm-project\build-release\bin), and writes the oracles'
rem outputs beside them with oracle.py: llvm-dis's IR (.ll),
rem llvm-bcanalyzer's dump (.dump.txt), and llvm-lto2's symbol table
rem (.symtab.txt). See testdata\bitcode\README.md.
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
if not defined LLVM_BIN set "LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin"
set "OUT=%~dp0..\..\testdata\bitcode"
if not exist "%OUT%" mkdir "%OUT%"
set "WORK=%~dp0..\..\target\fixtures\bitcode"
if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%"
cd /d "%WORK%" || exit /b 1
set "SRC=%~dp0"
set "CC=%LLVM_BIN%\clang.exe"
set "LINUX=--target=x86_64-unknown-linux-gnu"

rem Plain bitcode (clang -c -emit-llvm), at -O1 and -O0, with debug info.
"%CC%" %LINUX% -O1 -g -c -emit-llvm "%SRC%kinds.c" -o kinds.bc || exit /b 1
"%CC%" %LINUX% -O0 -g -c -emit-llvm "%SRC%shapes.cpp" -o shapes-linux.bc || exit /b 1
rem Windows: exceptions are funclets (catchswitch, catchpad, cleanuppad).
"%CC%" --target=x86_64-pc-windows-msvc -O1 -g -fexceptions -fcxx-exceptions -c -emit-llvm "%SRC%shapes.cpp" -o shapes-windows.bc || exit /b 1
rem ThinLTO's object: a summary, a string table, and a symbol table.
"%CC%" %LINUX% -O2 -flto=thin -c "%SRC%kinds.c" -o kinds-thin.bc || exit /b 1
rem Every instruction, type, and constant clang doesn't make, assembled by
rem opt (with no passes), which writes a symbol table where llvm-as doesn't.
"%LLVM_BIN%\opt.exe" "%SRC%every.ll" -o every.bc || exit /b 1
rem Darwin's LTO object: bitcode in its wrapper (0x0B17C0DE).
"%CC%" --target=x86_64-apple-macos11 -O1 -flto -c "%SRC%embed.c" -o embed-darwin.bc || exit /b 1
rem Bitcode embedded in each object format (-fembed-bitcode).
"%CC%" %LINUX% -O1 -fembed-bitcode -c "%SRC%embed.c" -o embed-elf.o || exit /b 1
"%CC%" --target=x86_64-apple-macos11 -O1 -fembed-bitcode -c "%SRC%embed.c" -o embed-macho.o || exit /b 1
"%CC%" --target=x86_64-pc-windows-msvc -O1 -fembed-bitcode -c "%SRC%embed.c" -o embed-coff.obj || exit /b 1
"%CC%" --target=wasm32-unknown-unknown -O1 -fembed-bitcode -c "%SRC%embed.c" -o embed-wasm.o || exit /b 1
rem Two modules in one file, and an archive of bitcode members (and an
rem ELF object with bitcode in it).
"%LLVM_BIN%\llvm-cat.exe" -b kinds-thin.bc every.bc -o multi.bc || exit /b 1
"%LLVM_BIN%\llvm-ar.exe" --format=gnu rcsD libbitcode.a kinds-thin.bc every.bc embed-elf.o || exit /b 1

for %%f in (kinds.bc shapes-linux.bc shapes-windows.bc kinds-thin.bc every.bc embed-darwin.bc embed-elf.o embed-macho.o embed-coff.obj embed-wasm.o multi.bc libbitcode.a) do copy /y %%f "%OUT%\%%f" >nul || exit /b 1
python "%SRC%oracle.py" "%LLVM_BIN%" "%OUT%" || exit /b 1
exit /b 0
