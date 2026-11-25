@echo off
rem Builds the PGO test program (pgo.cpp) with MSVC for x64, optimized by
rem profile-guided optimization: compiled with /GL, linked instrumented
rem (/GENPROFILE), trained by running it once, and linked again with the
rem profile (/USEPROFILE), as testdata\pgo\pgo.exe with its PDB.
rem The C runtime is the DLL (/MD), to keep the PDB small.
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set OUT=%~dp0..\..\testdata\pgo
if not exist "%OUT%" mkdir "%OUT%"
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
set WORK=%~dp0..\..\target\fixtures\pgo
rem Fresh each time: the linker updates an existing PDB in place.
if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%"
cd /d "%WORK%" || exit /b 1
cl /nologo /c /O2 /GL /MD /Zi /Fdpgo-c.pdb "%~dp0pgo.cpp" || exit /b 1
link /nologo /LTCG /GENPROFILE /DEBUG:FULL /INCREMENTAL:NO /Brepro pgo.obj /OUT:pgo.exe >nul || exit /b 1
rem The training: hot runs 2000 times, cold once, never not at all.
"%WORK%\pgo.exe" 1000 >nul || exit /b 1
link /nologo /LTCG /USEPROFILE /DEBUG:FULL /INCREMENTAL:NO /Brepro pgo.obj /OUT:pgo.exe /PDB:pgo.pdb >nul || exit /b 1
for %%f in (exe pdb) do copy /y pgo.%%f "%OUT%\pgo.%%f" >nul || exit /b 1
exit /b 0
