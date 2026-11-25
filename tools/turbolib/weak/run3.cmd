@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set LLD=%LLVM_BIN%\lld-link.exe
cl /nologo /c /Od /Z7 /Brepro main5.cpp main6.cpp >nul || exit /b 1
for %%m in (main5 main6) do (
  for %%l in (orig weak2) do (
    link /nologo /OUT:%%m-%%l.exe %%m.obj %%l.lib >%%m-%%l.link.txt 2>&1
    %LLD% /OUT:%%m-%%l-lld.exe %%m.obj %%l.lib libcmt.lib >%%m-%%l-lld.link.txt 2>&1
  )
)
