@echo off
rem Builds turbolib's phase 4 fixtures into testdata\coff\loose, from the
rem sources here, for turbonop-fold's tests of --fold=select-any and
rem --locals=copy (crates/link/fold/src/rewrite/loose_tests.rs) and for
rem check.py:
rem
rem   msvc-od-{a,b,c}.obj     cl /Od /Gy /Z7 (no /GF): string literals are
rem                           local $SG symbols in .rdata; static inline
rem                           and anonymous-namespace functions are local
rem                           COMDATs
rem   msvc-odnogy-{a,b,c}.obj cl /Od /Z7, without /Gy, as LLVM's Debug build
rem   msvc-o2-{a,b,c}.obj     cl /O2 /Gy /Z7 (string pooling, inlining)
rem   clang-od-{a,b,c}.obj    clang-cl /Od /Gy /Z7 -fno-addrsig
rem
rem a.cpp defines VARIANT 1, b.cpp and c.cpp 2 (shared.h's variant(),
rem selected, and refers() differ between a and the others).
rem
rem LLVM_BIN is where clang-cl is (the default is a Release build of
rem llvm-project at C:\jm\dev\llvm-project\build-release).
setlocal
rem (Run from R:, the repository mapped: see ..\..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\..\neutral.cmd" "%~f0" & exit /b)
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set SRC=%~dp0
set OUT=%~dp0..\..\..\testdata\coff\loose
set PATH=%PATH%;%ProgramFiles(x86)%\Microsoft Visual Studio\Installer
for /f "usebackq delims=" %%i in (`vswhere.exe -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
if not exist "%OUT%" mkdir "%OUT%" || exit /b 1
cd /d "%SRC%" || exit /b 1
set CL_=/nologo /c /std:c++17 /EHsc /GS- /Brepro /Z7
for %%f in (a b c) do (
  cl %CL_% /Od /Ob0 /Gy %%f.cpp /Fo:"%OUT%\msvc-od-%%f.obj" || exit /b 1
  cl %CL_% /Od /Ob0 %%f.cpp /Fo:"%OUT%\msvc-odnogy-%%f.obj" || exit /b 1
  cl %CL_% /O2 /Gy %%f.cpp /Fo:"%OUT%\msvc-o2-%%f.obj" || exit /b 1
  "%LLVM_BIN%\clang-cl.exe" %CL_% /Od /Gy -fno-addrsig %%f.cpp /Fo:"%OUT%\clang-od-%%f.obj" || exit /b 1
)
