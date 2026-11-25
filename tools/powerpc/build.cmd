@echo off
rem Writes turbonop-powerpc's tables (crates\isa\powerpc\src\tables.rs)
rem from LLVM's TableGen records of its PowerPC target, and the decoder's
rem golden file (crates\isa\powerpc\tests\golden.txt) from llvm-objdump's
rem text for encodings of each instruction. LLVM_BIN names an LLVM bin
rem directory (llvm-tblgen, llvm-mc, llvm-objcopy, llvm-objdump); LLVM_SRC
rem its llvm source directory.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build 1>&2 & exit /b 1)
if "%LLVM_SRC%"=="" (echo set LLVM_SRC to an LLVM source tree's llvm directory 1>&2 & exit /b 1)
set CRATE=%~dp0..\..\crates\isa\powerpc
"%LLVM_BIN%\llvm-tblgen.exe" --dump-json -I "%LLVM_SRC%\include" -I "%LLVM_SRC%\lib\Target\PowerPC" "%LLVM_SRC%\lib\Target\PowerPC\PPC.td" -o "%TEMP%\ppc.json" || exit /b 1
python "%~dp0extract.py" "%TEMP%\ppc.json" "%CRATE%\src\tables.rs" || exit /b 1
del "%TEMP%\ppc.json"
python "%~dp0oracle.py" "%LLVM_BIN%" "%CRATE%" || exit /b 1
