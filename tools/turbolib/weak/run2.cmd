@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set LLD=%LLVM_BIN%\lld-link.exe
python -I weak.py a || exit /b 1
lib /nologo /OUT:weak2.lib a.weak.obj b.obj x.obj || exit /b 1
dumpbin /nologo /linkermember:1 weak2.lib | findstr /i "shared_f"
for %%m in (main1 main2 main3 main4) do (
  link /nologo /OUT:%%m-weak2.exe %%m.obj weak2.lib >%%m-weak2.link.txt 2>&1
  %LLD% /OUT:%%m-weak2-lld.exe %%m.obj weak2.lib libcmt.lib >%%m-weak2-lld.link.txt 2>&1
  link /nologo /OPT:NOREF /OUT:%%m-weak2-noref.exe %%m.obj weak2.lib >%%m-weak2-noref.link.txt 2>&1
)
