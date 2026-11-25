@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set R=%LLVM_BIN%
echo == link.exe
link /nologo /OUT:m-link.exe main.obj big-llvm.lib
echo exit %errorlevel%
echo == lld-link
%R%\lld-link.exe /OUT:m-lld.exe main.obj big-llvm.lib libcmt.lib
echo exit %errorlevel%
