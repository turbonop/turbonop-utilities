@echo off
rem Builds the vtables test program (main.cpp) with MSVC for x64 and ARM64,
rem as testdata\vtables\vtables-<arch>.exe with its full PDB and a
rem stripped one (public symbols only, as Microsoft's symbol server serves
rem its own): vtables-<arch>.pdb and vtables-<arch>-stripped.pdb.
rem With RTTI (/GR, the default) and the C runtime as the DLL (/MD).
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set OUT=%~dp0..\..\testdata\vtables
if not exist "%OUT%" mkdir "%OUT%"
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call :build x64 amd64 || exit /b 1
call :build arm64 amd64_arm64 || exit /b 1
exit /b 0

:build
setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" %2 >nul 2>&1 || exit /b 1
set WORK=%~dp0..\..\target\fixtures\vtables-%1
rem Fresh each time: the linker updates an existing PDB in place.
if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%"
cd /d "%WORK%" || exit /b 1
cl /nologo /c /O1 /MD /Gy /GR /EHsc /Zi /std:c++20 /Fdvtables.pdb "%~dp0main.cpp" || exit /b 1
link /nologo /DEBUG:FULL /OPT:REF /OPT:ICF /INCREMENTAL:NO /Brepro /PDB:vtables-%1.pdb /PDBSTRIPPED:vtables-%1-stripped.pdb main.obj /OUT:vtables-%1.exe || exit /b 1
for %%f in (vtables-%1.exe vtables-%1.pdb vtables-%1-stripped.pdb) do copy /y %%f "%OUT%\%%f" >nul || exit /b 1
exit /b 0
