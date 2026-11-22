@echo off
rem Builds testdata\mingw: a PE image with DWARF, as clang --target=*-windows-gnu
rem (MinGW's toolchain) links one with lld, freestanding (no CRT): program.exe;
rem the same stripped, its DWARF in program.debug, which .gnu_debuglink names
rem (program-stripped.exe); and an ARM64 build (program-arm64.exe). And COFF
rem symbol tables: weak.c's objects (weak.o, weak-arm64.o; weak.obj, for
rem MSVC's target), and library.dll (exports, and its symbols); with
rem dumpbin's /symbols of each, and of program.exe (*.symbols.txt). And
rem program.c's objects, with DWARF a linker would relocate (program.o,
rem program-arm64.o). LLVM_BIN names an LLVM bin directory.
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
"%LLVM_BIN%\clang.exe" --target=x86_64-w64-windows-gnu -O1 -fno-ident -c "%~dp0weak.c" -o weak.o || exit /b 1
"%LLVM_BIN%\clang.exe" --target=aarch64-w64-windows-gnu -O1 -fno-ident -c "%~dp0weak.c" -o weak-arm64.o || exit /b 1
"%LLVM_BIN%\clang.exe" --target=x86_64-pc-windows-msvc -O1 -g -fno-ident -ffunction-sections -c "%~dp0weak.c" -o weak.obj || exit /b 1
"%LLVM_BIN%\clang.exe" --target=x86_64-w64-windows-gnu -O1 -ffreestanding -nostdlib -fno-ident -fuse-ld=lld -shared -Wl,-e,DllMainCRTStartup -Wl,--no-insert-timestamp "%~dp0library.c" -o library.dll || exit /b 1
set OF=-g -O1 -ffreestanding -fno-ident -ffunction-sections -fdata-sections -c
"%LLVM_BIN%\clang.exe" --target=x86_64-w64-windows-gnu %OF% "%~dp0program.c" -o program.o || exit /b 1
"%LLVM_BIN%\clang.exe" --target=aarch64-w64-windows-gnu %OF% "%~dp0program.c" -o program-arm64.o || exit /b 1
rem dumpbin's view of each symbol table, which hexc dump -symbols matches.
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
for %%f in (program.exe library.dll weak.o weak-arm64.o weak.obj) do (
  dumpbin /nologo /symbols %%f > %%f.symbols.txt || exit /b 1
)
