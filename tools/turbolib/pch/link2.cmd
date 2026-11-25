@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
for %%v in (noids ids) do link /nologo /DEBUG:FULL /OUT:%%v.exe /PDB:%%v.pdb main.obj a.%%v.user.obj b.obj a.%%v.pch.obj
