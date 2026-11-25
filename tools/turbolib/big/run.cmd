@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set R=%LLVM_BIN%
cl /nologo /c main.cpp >nul
echo == lib.exe, 5 x 1.1 GiB
lib /nologo /OUT:big-lib.lib big1.obj big2.obj big3.obj big4.obj big5.obj
echo exit %errorlevel%
echo == llvm-lib, 5 x 1.1 GiB
%R%\llvm-lib.exe /OUT:big-llvm.lib big1.obj big2.obj big3.obj big4.obj big5.obj
echo exit %errorlevel%
echo == lib.exe, 3 x 1.1 GiB (under 4 GiB)
lib /nologo /OUT:big3-lib.lib big1.obj big2.obj big3.obj
echo exit %errorlevel%
