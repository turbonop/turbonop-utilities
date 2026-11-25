@echo off
rem Builds testdata\arm32: tools\arm32\program.c for 32-bit ARM (Thumb-2),
rem freestanding: program-linux (ELF, armhf, with DWARF, by ld.lld) and
rem program.exe (Windows on ARM, with program.pdb, by lld-link). LLVM_BIN
rem names an LLVM bin directory. And eh-linux (eh.c): its exception tables.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build or release 1>&2 & exit /b 1)
set OUT=%~dp0..\..\testdata\arm32
if not exist "%OUT%" mkdir "%OUT%"
cd /d "%OUT%" || exit /b 1
set CF=-O2 -g -ffreestanding -fno-stack-protector -fno-ident -ffunction-sections -fdata-sections -fdebug-compilation-dir=.
"%LLVM_BIN%\clang.exe" --target=armv7a-linux-gnueabihf -mthumb %CF% -fno-asynchronous-unwind-tables -nostdlib -fuse-ld=lld -Wl,-e,start -Wl,--build-id=sha1 "%~dp0program.c" -o program-linux || exit /b 1
"%LLVM_BIN%\clang.exe" --target=thumbv7-pc-windows-msvc %CF% -gcodeview -c "%~dp0program.c" -o program.obj || exit /b 1
"%LLVM_BIN%\lld-link.exe" /nologo /entry:start /subsystem:console /nodefaultlib /debug /pdbaltpath:%%_PDB%% /Brepro /out:program.exe program.obj || exit /b 1
del program.obj
rem Its exception tables: eh-linux (eh.c), with DWARF.
"%LLVM_BIN%\clang.exe" --target=armv7a-linux-gnueabihf -mthumb %CF% -fexceptions -nostdlib -fuse-ld=lld -Wl,-e,start -Wl,--build-id=sha1 -Wl,--unresolved-symbols=ignore-all "%~dp0eh.c" -o eh-linux || exit /b 1
