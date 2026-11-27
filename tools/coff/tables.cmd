@echo off
rem Builds turbolib's side-table test objects into testdata\coff\tables, from
rem tables\one.cpp and tables\two.cpp (which share tables\shared.h's inline
rem functions, so each object has a copy of each, with its table entries):
rem
rem   msvc-x64-cf-*.obj        cl /Gy /guard:cf /guard:ehcont: .gfids$y,
rem                            .giats$y, .gljmp$y, .gehcont$y, each in a
rem                            COMDAT associated with its function
rem   msvc-x64-cf-nogy-*.obj   the same with /Gy-: one table of each kind
rem                            for the code outside COMDATs
rem   clang-x64-cf-*.obj       clang-cl /guard:cf /guard:ehcont: one table
rem                            of each kind per object
rem   msvc-arm64-cf-*.obj      cl for ARM64, /Gy /guard:cf /guard:ehcont
rem   msvc-x86-*.obj           cl for x86, /Gy /guard:cf: .sxdata, and FPO
rem                            (.debug$F) for every function
rem   msvc-x86-nogy-*.obj      the same with /Gy-: FPO outside COMDATs too
rem   clang-x86-*.obj          clang-cl -m32 /guard:cf: .sxdata
rem   msvc-x64-voltbl-*.obj    cl /Gy /volatileMetadata: .voltbl in COMDATs
rem   msvc-x64-voltbl-nogy-*.obj  /Gy-: .voltbl records outside COMDATs
rem   msvc-x64-retpoline-*.obj    cl /Gy /d2guardretpoline: .retplne
rem
rem LLVM_BIN is where clang-cl is (the default is a Release build of
rem llvm-project at C:\jm\dev\llvm-project\build-release).
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set SRC=%~dp0tables
set OUT=%~dp0..\..\testdata\coff\tables
set PATH=%PATH%;%ProgramFiles(x86)%\Microsoft Visual Studio\Installer
for /f "usebackq delims=" %%i in (`vswhere.exe -latest -property installationPath`) do set VS=%%i
if not exist "%OUT%" mkdir "%OUT%" || exit /b 1
cd /d "%OUT%" || exit /b 1
set CL_FLAGS=/nologo /c /O2 /EHsc /Brepro

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
call :both msvc-x64-cf cl %CL_FLAGS% /Gy /guard:cf /guard:ehcont || exit /b 1
call :both msvc-x64-cf-nogy cl %CL_FLAGS% /Gy- /guard:cf /guard:ehcont || exit /b 1
call :both clang-x64-cf "%LLVM_BIN%\clang-cl" %CL_FLAGS% /guard:cf /guard:ehcont || exit /b 1
call :both msvc-x64-voltbl cl %CL_FLAGS% /Gy /volatileMetadata || exit /b 1
call :both msvc-x64-voltbl-nogy cl %CL_FLAGS% /Gy- /volatileMetadata || exit /b 1
call :both msvc-x64-retpoline cl %CL_FLAGS% /Gy /d2guardretpoline || exit /b 1
endlocal

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64_arm64 >nul 2>&1 || exit /b 1
call :both msvc-arm64-cf cl %CL_FLAGS% /Gy /guard:cf /guard:ehcont || exit /b 1
endlocal

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64_x86 >nul 2>&1 || exit /b 1
call :both msvc-x86 cl %CL_FLAGS% /Gy /guard:cf || exit /b 1
call :both msvc-x86-nogy cl %CL_FLAGS% /Gy- /guard:cf || exit /b 1
call :both clang-x86 "%LLVM_BIN%\clang-cl" -m32 %CL_FLAGS% /guard:cf || exit /b 1
endlocal
exit /b 0

rem :both name compiler flags...: name-one.obj and name-two.obj.
:both
set NAME=%~1
shift
set CMD=
:args
if "%~1"=="" goto run
set CMD=%CMD% %1
shift
goto args
:run
%CMD% "%SRC%\one.cpp" /Fo:%NAME%-one.obj >nul || exit /b 1
%CMD% "%SRC%\two.cpp" /Fo:%NAME%-two.obj >nul || exit /b 1
exit /b 0
