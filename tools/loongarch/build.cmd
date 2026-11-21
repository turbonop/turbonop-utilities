@echo off
rem Writes hexcavator-loongarch's tables (crates\isa\loongarch\src\tables.rs)
rem from LLVM's TableGen records of its LoongArch target, and the decoder's
rem golden file (crates\isa\loongarch\tests\golden.txt) from llvm-objdump's
rem text for encodings of each instruction. LLVM_BIN names an LLVM bin
rem directory (llvm-tblgen, llvm-mc, llvm-objdump); LLVM_SRC its llvm source
rem directory.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build 1>&2 & exit /b 1)
if "%LLVM_SRC%"=="" (echo set LLVM_SRC to an LLVM source tree's llvm directory 1>&2 & exit /b 1)
set CRATE=%~dp0..\..\crates\isa\loongarch
"%LLVM_BIN%\llvm-tblgen.exe" --dump-json -I "%LLVM_SRC%\include" -I "%LLVM_SRC%\lib\Target\LoongArch" "%LLVM_SRC%\lib\Target\LoongArch\LoongArch.td" -o "%TEMP%\loongarch.json" || exit /b 1
python "%~dp0extract.py" "%TEMP%\loongarch.json" "%CRATE%\src\tables.rs" || exit /b 1
del "%TEMP%\loongarch.json"
python "%~dp0oracle.py" "%LLVM_BIN%" "%CRATE%" || exit /b 1
