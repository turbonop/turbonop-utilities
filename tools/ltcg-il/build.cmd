@echo off
rem Builds Hexcavator's LTCG IL test inputs into testdata\ltcg-il, with
rem MSVC (/GL, /Brepro): records.obj (x64, records.cpp, C++20),
rem records-a64.obj (ARM64, the same), records-c.obj (x64, records.c), and
rem records-x86.obj (x86, records.c); records.lib (records.obj and
rem records-c.obj). Beside them, lib.exe's symbol index of a library of
rem each object (dumpbin /linkermember:2: *.linkermember.txt), which the
rem public definitions hexcavator finds match.
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set OUT=%~dp0..\..\testdata\ltcg-il
if not exist "%OUT%" mkdir "%OUT%"
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call :build amd64 records.obj "/std:c++20 /EHsc %~dp0records.cpp" || exit /b 1
call :build amd64 records-c.obj "%~dp0records.c" || exit /b 1
call :build amd64_arm64 records-a64.obj "/std:c++20 /EHsc %~dp0records.cpp" || exit /b 1
call :build x86 records-x86.obj "%~dp0records.c" || exit /b 1
setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
cd /d "%OUT%" || exit /b 1
lib /nologo /Brepro /out:records.lib records.obj records-c.obj || exit /b 1
endlocal
exit /b 0

rem :build <vcvarsall target> <object> <cl arguments>: the object, and
rem lib.exe's index of a library of it.
:build
setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" %1 >nul 2>&1 || exit /b 1
cd /d "%OUT%" || exit /b 1
cl /nologo /c /O1 /GL /GS- /Brepro %~3 /Fo:%2 || exit /b 1
lib /nologo /Brepro /out:%~n2.tmp.lib %2 || exit /b 1
dumpbin /nologo /linkermember:2 %~n2.tmp.lib > %~n2.linkermember.txt || exit /b 1
del %~n2.tmp.lib
exit /b 0
