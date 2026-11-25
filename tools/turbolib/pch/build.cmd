@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
cl /nologo /c /Od /Z7 /Brepro /Yc"shared.h" pch.cpp /Fppch.pch /Fo:pch.obj || exit /b 1
cl /nologo /c /Od /Z7 /Brepro /Yu"shared.h" user.cpp /Fppch.pch /Fo:user.obj || exit /b 1
cl /nologo /c /Od /Z7 /Brepro a.cpp b.cpp main.cpp || exit /b 1
