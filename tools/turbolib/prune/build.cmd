@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set LLD=%LLVM_BIN%\lld-link.exe
python -I make.py || exit /b 1
cl /nologo /c /Od /Gy /Z7 /Brepro a.cpp main.cpp main2.cpp || exit /b 1
cl /nologo /c /Od /Gy /Z7 /Brepro /Yc"big.h" pch.cpp /Fpbig.pch /Fo:pch.obj || exit /b 1
cl /nologo /c /Od /Gy /Z7 /Brepro /Yu"big.h" user.cpp /Fpbig.pch /Fo:user.obj || exit /b 1
for %%o in (REF NOREF) do (
  link /nologo /DEBUG:FULL /OPT:%%o /OUT:a-%%o.exe /PDB:a-%%o.pdb main.obj a.obj
  %LLD% /DEBUG:FULL /OPT:%%o /OUT:a-%%o-lld.exe /PDB:a-%%o-lld.pdb main.obj a.obj libcmt.lib
  link /nologo /DEBUG:FULL /OPT:%%o /OUT:p-%%o.exe /PDB:p-%%o.pdb main2.obj user.obj pch.obj
  %LLD% /DEBUG:FULL /OPT:%%o /OUT:p-%%o-lld.exe /PDB:p-%%o-lld.pdb main2.obj user.obj pch.obj libcmt.lib
)
