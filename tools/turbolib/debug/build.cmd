@echo off
rem Builds turbolib's debug-information fixtures into testdata\coff\fold,
rem from the sources here, for turbonop-fold's tests of synthesized
rem members' CodeView (crates/link/fold/src/rewrite/debug/tests.rs):
rem
rem   msvc-a.obj, msvc-b.obj    cl /Od /Z7: per-function .debug$S, types
rem                             that a and b share (shared.h), b's files in
rem                             another order (other.h first)
rem   msvc-c.obj, msvc-d.obj    cl /O2 /Z7: deep() and shallow() inline
rem                             from six headers (inline sites that change
rem                             files, and inlinee lines)
rem   msvc-pch.obj              cl /Yc /Z7: a PCH object (.debug$P)
rem   msvc-u1.obj, msvc-u2.obj  cl /Yu /Z7: its users (LF_PRECOMP)
rem   msvc-zi-a.obj, -b.obj     cl /Zi: types in a type server (msvc-zi.pdb)
rem   clang-a.obj ... clang-d.obj  clang-cl, as msvc-a to msvc-d, with
rem                             -fno-addrsig (.llvm_addrsig is a side table)
rem
rem LLVM_BIN is where clang-cl is (the default is a Release build of
rem llvm-project at C:\jm\dev\llvm-project\build-release).
setlocal
rem (Run from R:, the repository mapped: see ..\..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\..\neutral.cmd" "%~f0" & exit /b)
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set SRC=%~dp0
set OUT=%~dp0..\..\..\testdata\coff\fold
set PATH=%PATH%;%ProgramFiles(x86)%\Microsoft Visual Studio\Installer
for /f "usebackq delims=" %%i in (`vswhere.exe -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
if not exist "%OUT%" mkdir "%OUT%" || exit /b 1
cd /d "%SRC%" || exit /b 1
set CL_=/nologo /c /std:c++17 /EHsc /Gy /GS- /Brepro
set CLANG_=/nologo /c /std:c++17 /EHsc /Gy /GS- /Brepro -fno-addrsig
cl %CL_% /Od /Ob0 /Z7 a.cpp /Fo:"%OUT%\msvc-a.obj" || exit /b 1
cl %CL_% /Od /Ob0 /Z7 b.cpp /Fo:"%OUT%\msvc-b.obj" || exit /b 1
cl %CL_% /O2 /Z7 c.cpp /Fo:"%OUT%\msvc-c.obj" || exit /b 1
cl %CL_% /O2 /Z7 d.cpp /Fo:"%OUT%\msvc-d.obj" || exit /b 1
cl %CL_% /Od /Ob0 /Z7 /Yc"pch.h" /Fp:"%OUT%\msvc.pch" pch.cpp /Fo:"%OUT%\msvc-pch.obj" || exit /b 1
cl %CL_% /Od /Ob0 /Z7 /Yu"pch.h" /Fp:"%OUT%\msvc.pch" u1.cpp /Fo:"%OUT%\msvc-u1.obj" || exit /b 1
cl %CL_% /Od /Ob0 /Z7 /Yu"pch.h" /Fp:"%OUT%\msvc.pch" u2.cpp /Fo:"%OUT%\msvc-u2.obj" || exit /b 1
del "%OUT%\msvc.pch"
cl %CL_% /Od /Ob0 /Zi /Fd:"%OUT%\msvc-zi.pdb" a.cpp /Fo:"%OUT%\msvc-zi-a.obj" || exit /b 1
cl %CL_% /Od /Ob0 /Zi /Fd:"%OUT%\msvc-zi.pdb" b.cpp /Fo:"%OUT%\msvc-zi-b.obj" || exit /b 1
"%LLVM_BIN%\clang-cl.exe" %CLANG_% /Od /Z7 a.cpp /Fo:"%OUT%\clang-a.obj" || exit /b 1
"%LLVM_BIN%\clang-cl.exe" %CLANG_% /Od /Z7 b.cpp /Fo:"%OUT%\clang-b.obj" || exit /b 1
"%LLVM_BIN%\clang-cl.exe" %CLANG_% /O2 /Z7 c.cpp /Fo:"%OUT%\clang-c.obj" || exit /b 1
"%LLVM_BIN%\clang-cl.exe" %CLANG_% /O2 /Z7 d.cpp /Fo:"%OUT%\clang-d.obj" || exit /b 1
