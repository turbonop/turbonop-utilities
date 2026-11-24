@echo off
rem Writes turbonop-riscv's tables (crates\isa\riscv\src\tables.rs) from
rem LLVM's TableGen records of its RISC-V target, and the decoder's golden
rem file (crates\isa\riscv\tests\golden.txt) from llvm-objdump's text for
rem encodings of each instruction. LLVM_BIN names an LLVM bin directory
rem (llvm-tblgen, llvm-mc, llvm-objdump); LLVM_SRC its llvm source
rem directory. The records' dump is large, and slow to make.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build 1>&2 & exit /b 1)
if "%LLVM_SRC%"=="" (echo set LLVM_SRC to an LLVM source tree's llvm directory 1>&2 & exit /b 1)
set CRATE=%~dp0..\..\crates\isa\riscv
"%LLVM_BIN%\llvm-tblgen.exe" --dump-json -I "%LLVM_SRC%\include" -I "%LLVM_SRC%\lib\Target\RISCV" "%LLVM_SRC%\lib\Target\RISCV\RISCV.td" -o "%TEMP%\riscv.json" || exit /b 1
python "%~dp0extract.py" "%TEMP%\riscv.json" "%CRATE%\src\tables.rs" || exit /b 1
del "%TEMP%\riscv.json"
python "%~dp0oracle.py" "%LLVM_BIN%" "%CRATE%" || exit /b 1
