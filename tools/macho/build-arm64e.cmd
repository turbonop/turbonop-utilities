@echo off
rem Builds Hexcavator's arm64e (pointer authentication) Mach-O test inputs
rem into testdata\macho with clang and ld64.lld from LLVM (LLVM_BIN names
rem its bin directory), from the sources build.cmd builds arm64 and x86_64
rem from, so that the tests can check arm64e's reading against arm64's:
rem - pointers-arm64e.dylib: chained fixups (DYLD_CHAINED_PTR_ARM64E_USERLAND24),
rem   function pointers signed (rebases and binds), with llvm-objdump's
rem   --dyld-info listing;
rem - program-arm64e and program-arm64e-universal (arm64 and arm64e): code
rem   that signs its return addresses (pacibsp, retab), with llvm-objdump's
rem   disassembly, llvm-readobj's view, and dsymutil's DWARF (the bare
rem   file);
rem - unwind-arm64e.dylib: compact unwind, with llvm-objdump's;
rem - objc-arm64e.dylib: Objective-C metadata, its method lists' pointers
rem   signed;
rem - ptrauth-arm64e.dylib: __ptrauth-qualified pointers (ptrauth.c), with
rem   dsymutil's DWARF (the bare file) and llvm-dwarfdump's view of it.
setlocal enabledelayedexpansion
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build or release 1>&2 & exit /b 1)
set OUT=%~dp0..\..\testdata\macho
if not exist "%OUT%" mkdir "%OUT%"
cd /d "%OUT%" || exit /b 1
set TARGET=--target=arm64e-apple-macos14
set LINK=-arch arm64e -platform_version macos 14.0 14.0
set CFLAGS=-O1 -g -ffreestanding -fno-stack-protector -fno-ident -ffile-prefix-map=%~dp0= -fdebug-compilation-dir=.

"%LLVM_BIN%\clang.exe" %TARGET% -O1 -ffreestanding -fno-stack-protector -fno-ident -c "%~dp0pointers.c" -o pointers-arm64e.o || exit /b 1
"%LLVM_BIN%\ld64.lld.exe" %LINK% -dylib -undefined dynamic_lookup -install_name @rpath/libpointers.dylib pointers-arm64e.o -o pointers-arm64e.dylib || exit /b 1
del pointers-arm64e.o
"%LLVM_BIN%\llvm-objdump.exe" --macho --dyld-info pointers-arm64e.dylib > pointers-arm64e.dylib.fixups.txt || exit /b 1

"%LLVM_BIN%\clang.exe" %TARGET% %CFLAGS% -c "%~dp0program.c" -o program-arm64e.o || exit /b 1
"%LLVM_BIN%\clang.exe" %TARGET% %CFLAGS% -c "%~dp0..\elf\second.c" -o second-arm64e.o || exit /b 1
"%LLVM_BIN%\ld64.lld.exe" %LINK% -oso_prefix . -e _start program-arm64e.o second-arm64e.o -o program-arm64e || exit /b 1
"%LLVM_BIN%\dsymutil.exe" program-arm64e -o program-arm64e.dSYM || exit /b 1
copy /y program-arm64e.dSYM\Contents\Resources\DWARF\program-arm64e program-arm64e.dwarf >nul || exit /b 1
rmdir /s /q program-arm64e.dSYM
del program-arm64e.o second-arm64e.o
"%LLVM_BIN%\llvm-objdump.exe" -d --no-show-raw-insn program-arm64e > program-arm64e.objdump.txt || exit /b 1
"%LLVM_BIN%\llvm-readobj.exe" --file-headers --sections --symbols --macho-segment --macho-dysymtab --macho-version-min program-arm64e > program-arm64e.readobj.txt || exit /b 1
"%LLVM_BIN%\llvm-lipo.exe" -create program-arm64 program-arm64e -output program-arm64e-universal || exit /b 1

"%LLVM_BIN%\clang.exe" %TARGET% -O1 -fno-stack-protector -fno-ident -c "%~dp0unwind.c" -o unwind-arm64e.o || exit /b 1
"%LLVM_BIN%\ld64.lld.exe" %LINK% -dylib -undefined dynamic_lookup -install_name @rpath/libunwind-test.dylib unwind-arm64e.o -o unwind-arm64e.dylib || exit /b 1
del unwind-arm64e.o
"%LLVM_BIN%\llvm-objdump.exe" --macho --unwind-info --function-starts unwind-arm64e.dylib > unwind-arm64e.dylib.unwind.txt || exit /b 1

"%LLVM_BIN%\clang.exe" %TARGET% -O1 -fno-stack-protector -fno-ident -fobjc-runtime=macosx-14.0 -c "%~dp0objc.m" -o objc-arm64e.o || exit /b 1
"%LLVM_BIN%\ld64.lld.exe" %LINK% -dylib -undefined dynamic_lookup -install_name @rpath/libobjc-test.dylib objc-arm64e.o -o objc-arm64e.dylib || exit /b 1
del objc-arm64e.o

"%LLVM_BIN%\clang.exe" %TARGET% %CFLAGS% -c "%~dp0ptrauth.c" -o ptrauth-arm64e.o || exit /b 1
"%LLVM_BIN%\ld64.lld.exe" %LINK% -dylib -oso_prefix . -install_name @rpath/libptrauth.dylib ptrauth-arm64e.o -o ptrauth-arm64e.dylib || exit /b 1
"%LLVM_BIN%\dsymutil.exe" ptrauth-arm64e.dylib -o ptrauth-arm64e.dSYM || exit /b 1
copy /y ptrauth-arm64e.dSYM\Contents\Resources\DWARF\ptrauth-arm64e.dylib ptrauth-arm64e.dwarf >nul || exit /b 1
rmdir /s /q ptrauth-arm64e.dSYM
del ptrauth-arm64e.o
"%LLVM_BIN%\llvm-dwarfdump.exe" --debug-info ptrauth-arm64e.dwarf > ptrauth-arm64e.dwarf.dwarfdump.txt || exit /b 1
