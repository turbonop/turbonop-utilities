@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set LLD=%LLVM_BIN%\lld-link.exe
cl /nologo /c /Od /Z7 /Brepro a.cpp b.cpp x.cpp main1.cpp main2.cpp main3.cpp main4.cpp >nul || exit /b 1
python -I weak.py b || exit /b 1
lib /nologo /OUT:orig.lib a.obj b.obj || exit /b 1
lib /nologo /OUT:weak.lib a.obj b.weak.obj x.obj || exit /b 1
for %%m in (main1 main2 main3 main4) do (
  for %%l in (orig weak) do (
    link /nologo /OUT:%%m-%%l.exe %%m.obj %%l.lib >%%m-%%l.link.txt 2>&1
    %LLD% /OUT:%%m-%%l-lld.exe %%m.obj %%l.lib libcmt.lib >%%m-%%l-lld.link.txt 2>&1
  )
)
