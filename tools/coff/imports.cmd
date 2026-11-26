@echo off
rem Builds the import-library writer's test inputs into testdata\coff\imports:
rem for each line of imports\cases.txt (name|tool|arguments), what lib.exe
rem (with /Brepro) or llvm-lib writes of the .def files in imports\ (and of
rem x86 objects: archives\decorated.c and imports\exports.c): the library,
rem and lib.exe's export files. turbolib's tests run the same lines.
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set SRC=%~dp0imports
set OUT=%~dp0..\..\testdata\coff\imports
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
set WORK=R:\target\fixtures\imports
if exist "%WORK%" rd /s /q "%WORK%"
mkdir "%WORK%" || exit /b 1
cd /d "%WORK%" || exit /b 1
if exist "%OUT%" rd /s /q "%OUT%"
mkdir "%OUT%" || exit /b 1
copy "%SRC%\*.def" . >nul || exit /b 1
copy "%~dp0archives\def\decorated.def" . >nul || exit /b 1
set X86CL="%VCToolsInstallDir%bin\Hostx64\x86\cl.exe"
%X86CL% /nologo /c /O1 /GS- /Brepro "%~dp0archives\decorated.c" /Fo:decorated.obj >nul || exit /b 1
%X86CL% /nologo /c /O1 /GS- /Brepro "%SRC%\exports.c" /Fo:exports.obj >nul || exit /b 1
for /f "usebackq tokens=1,2,* delims=|" %%a in ("%SRC%\cases.txt") do (
  if "%%b"=="lib" (lib %%c >nul) else ("%LLVM_BIN%\llvm-lib.exe" %%c)
)
copy *.lib "%OUT%" >nul || exit /b 1
copy *.exp "%OUT%" >nul || exit /b 1
copy *.obj "%OUT%" >nul || exit /b 1
exit /b 0
