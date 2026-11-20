@echo off
rem Builds testdata\pe-exports: exp.dll (many exports, with exp.pdb) and
rem app.exe, which imports two of them, by clang and lld-link, freestanding.
rem LLVM_BIN names an LLVM bin directory.
setlocal
if "%LLVM_BIN%"=="" (echo set LLVM_BIN to the bin directory of an LLVM build or release 1>&2 & exit /b 1)
set OUT=%~dp0..\..\testdata\pe-exports
if not exist "%OUT%" mkdir "%OUT%"
cd /d "%OUT%" || exit /b 1
set CF=--target=x86_64-pc-windows-msvc -O2 -g -gcodeview -fno-stack-protector -fdebug-compilation-dir=.
"%LLVM_BIN%\clang.exe" %CF% -c "%~dp0exp.c" -o exp.obj || exit /b 1
"%LLVM_BIN%\lld-link.exe" /nologo /dll /noentry /nodefaultlib /debug /pdbaltpath:%%_PDB%% /Brepro /out:exp.dll /implib:exp.lib exp.obj || exit /b 1
"%LLVM_BIN%\clang.exe" %CF% -c "%~dp0app.c" -o app.obj || exit /b 1
"%LLVM_BIN%\lld-link.exe" /nologo /entry:start /subsystem:console /nodefaultlib /debug /pdbaltpath:%%_PDB%% /Brepro /out:app.exe app.obj exp.lib || exit /b 1
del exp.obj app.obj exp.lib exp.exp app.pdb 2>nul
