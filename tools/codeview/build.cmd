@echo off
rem Builds turbonop-codeview's object fixtures into testdata\codeview: objects
rem whose CodeView has the record kinds LLVM's /Z7 build lacks, for the
rem fixtures test (crates\symbols\codeview\tests\it\objects.rs). From
rem kinds.cpp, with MSVC and clang-cl, optimized (live ranges, inline sites,
rem jump tables), with /GS (frame cookies):
rem   kinds-msvc-x64.obj, kinds-msvc-x64-od.obj (/Od /RTC1 /JMC),
rem   kinds-msvc-x86.obj (/Oy: frame data), kinds-msvc-arm64.obj,
rem   kinds-clang-x64.obj (with a secure hot-patch function),
rem   kinds-clang-x86.obj, kinds-clang-arm64.obj;
rem kinds-masm.obj (ml64 /Zi), managed.obj (C++/CLI, /clr), and a
rem precompiled header and its user, pch.obj (/Yc) and pch-user.obj (/Yu).
rem LLVM_BIN is clang-cl's directory (default
rem C:\jm\dev\llvm-project\build-release\bin).
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set OUT=%~dp0..\..\testdata\codeview
if not exist "%OUT%" mkdir "%OUT%"
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
set WORK=%~dp0..\..\target\fixtures\codeview
if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%"
cd /d "%WORK%" || exit /b 1
set SRC=%~dp0kinds.cpp
set CL_FLAGS=/nologo /c /Z7 /GS /Gy /EHsc /Brepro
set CLANG="%LLVM_BIN%\clang-cl.exe" /nologo /c /O2 /Z7 /GS /Brepro

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
cl %CL_FLAGS% /O2 "%SRC%" /Fo:kinds-msvc-x64.obj || exit /b 1
cl %CL_FLAGS% /Od /RTC1 /JMC "%SRC%" /Fo:kinds-msvc-x64-od.obj || exit /b 1
ml64 /nologo /c /Zi /Fo kinds-masm.obj "%~dp0kinds.asm" || exit /b 1
cl /nologo /c /clr /Z7 /Brepro "%~dp0managed.cpp" /Fo:managed.obj || exit /b 1
copy /y "%~dp0pch.h" . >nul || exit /b 1
cl %CL_FLAGS% /Od /Ycpch.h /Fppch.pch "%~dp0pch.cpp" /Fo:pch.obj || exit /b 1
cl %CL_FLAGS% /Od /Yupch.h /Fppch.pch "%~dp0pch-user.cpp" /Fo:pch-user.obj || exit /b 1
rem clang-cl, with the MSVC headers of the environment above.
%CLANG% /clang:-fms-secure-hotpatch-functions-list=sum_of_squares "%SRC%" /Fo:kinds-clang-x64.obj || exit /b 1
%CLANG% --target=i686-pc-windows-msvc "%SRC%" /Fo:kinds-clang-x86.obj || exit /b 1
%CLANG% --target=aarch64-pc-windows-msvc "%SRC%" /Fo:kinds-clang-arm64.obj || exit /b 1
endlocal

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64_x86 >nul 2>&1 || exit /b 1
cl %CL_FLAGS% /O2 /Oy "%SRC%" /Fo:kinds-msvc-x86.obj || exit /b 1
endlocal

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64_arm64 >nul 2>&1 || exit /b 1
cl %CL_FLAGS% /O2 "%SRC%" /Fo:kinds-msvc-arm64.obj || exit /b 1
endlocal

for %%f in (*.obj) do copy /y %%f "%OUT%\%%f" >nul || exit /b 1
exit /b 0
