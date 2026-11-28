@echo off
rem Writes turbonop-mips's tables (crates\isa\mips\src\generated\tables.rs, and
rem generated\micromips.rs, microMIPS's) from LLVM's TableGen records of its MIPS
rem target, and the decoder's golden files (crates\isa\mips\tests\golden.txt
rem and golden-micromips.txt) from llvm-objdump's text for encodings of
rem each instruction. LLVM_BIN names an LLVM bin directory
rem (llvm-tblgen, llvm-mc, llvm-objcopy, llvm-objdump); LLVM_SRC its llvm
rem source directory.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build 1>&2 & exit /b 1)
if "%LLVM_SRC%"=="" (echo set LLVM_SRC to an LLVM source tree's llvm directory 1>&2 & exit /b 1)
set CRATE=%~dp0..\..\crates\isa\mips
"%LLVM_BIN%\llvm-tblgen.exe" --dump-json -I "%LLVM_SRC%\include" -I "%LLVM_SRC%\lib\Target\Mips" "%LLVM_SRC%\lib\Target\Mips\Mips.td" -o "%TEMP%\mips.json" || exit /b 1
python "%~dp0extract.py" "%TEMP%\mips.json" "%CRATE%\src\generated\tables.rs" "%CRATE%\src\generated\micromips.rs" || exit /b 1
del "%TEMP%\mips.json"
python "%~dp0oracle.py" "%LLVM_BIN%" "%CRATE%" || exit /b 1
