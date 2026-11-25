@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set LLD=%LLVM_BIN%\lld-link.exe
echo == baseline
link /nologo /DEBUG:FULL /OUT:base.exe /PDB:base.pdb main.obj a.obj b.obj || exit /b 1
%LLD% /DEBUG:FULL /OUT:base-lld.exe /PDB:base-lld.pdb main.obj a.obj b.obj libcmt.lib || exit /b 1
echo == A: full (a's types all in a.pch.obj), objects on the command line
link /nologo /DEBUG:FULL /OUT:full.exe /PDB:full.pdb main.obj a.user.obj b.obj a.pch.obj
%LLD% /DEBUG:FULL /OUT:full-lld.exe /PDB:full-lld.pdb main.obj a.user.obj b.obj a.pch.obj libcmt.lib
echo == B: share (a and b share a prefix in ab.pch.obj), through a library
lib /nologo /OUT:ab.lib a.share.obj b.share.obj ab.pch.obj || exit /b 1
link /nologo /DEBUG:FULL /OUT:share.exe /PDB:share.pdb main.obj ab.lib
%LLD% /DEBUG:FULL /OUT:share-lld.exe /PDB:share-lld.pdb main.obj ab.lib libcmt.lib
echo == B with /DEBUG:GHASH (lld)
%LLD% /DEBUG:GHASH /OUT:share-ghash.exe /PDB:share-ghash.pdb main.obj ab.lib libcmt.lib
echo == B, link.exe /DEBUG:FASTLINK
link /nologo /DEBUG:FASTLINK /OUT:share-fast.exe /PDB:share-fast.pdb main.obj ab.lib
echo == run
for %%e in (base full full-lld share share-lld share-ghash) do (echo %%e: & .\%%e.exe & echo.)
