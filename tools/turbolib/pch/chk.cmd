@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set LLD=%LLVM_BIN%\lld-link.exe
for %%v in (stale zeroed) do (
  echo == %%v: link.exe
  link /nologo /DEBUG /OUT:chk-%%v.exe main.obj a.%%v.obj b.obj
  echo exit %errorlevel%
  echo == %%v: lld-link
  %LLD% /DEBUG /OUT:chk-%%v-lld.exe main.obj a.%%v.obj b.obj libcmt.lib
  echo exit %errorlevel%
)
