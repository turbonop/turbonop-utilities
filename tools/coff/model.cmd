@echo off
rem Builds turbonop-coff's test objects into testdata\coff\model, from the
rem sources in model\: what each compiler and assembler writes, for the
rem layouts, string tables, and auxiliary records each one has.
rem
rem   msvc-x64.obj      cl, x64: COMDATs, associative sections, /Z7
rem   msvc-x86.obj      cl, x86: decorated names, .sxdata
rem   msvc-arm64ec.obj  cl /arm64EC: anti-dependency weak externals,
rem                     long section names in MSVC's string table order
rem   msvc-clr.obj      cl /clr: CLR tokens
rem   clang-x64.obj     clang-cl: LLVM's layout and tail-merged strings
rem   clang-dwarf.obj   clang-cl -gdwarf: long .debug_* section names
rem   mc-x64.obj        llvm-mc, from model.s: every COMDAT selection,
rem                     weak aliases, common symbols, section relocations
rem   ml64.obj          ml64: relocations at even offsets (padding)
rem   armasm64.obj      armasm64 -g: function definitions, .bf, .ef, and a
rem                     multi-record .file
rem   res-x64.obj       cvtres: a string table whose size says 0
rem
rem LLVM_BIN is where clang-cl and llvm-mc are (the default is a Release
rem build of llvm-project at C:\jm\dev\llvm-project\build-release).
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set SRC=%~dp0model
set OUT=%~dp0..\..\testdata\coff\model
set PATH=%PATH%;%ProgramFiles(x86)%\Microsoft Visual Studio\Installer
for /f "usebackq delims=" %%i in (`vswhere.exe -latest -property installationPath`) do set VS=%%i
if not exist "%OUT%" mkdir "%OUT%" || exit /b 1
cd /d "%OUT%" || exit /b 1
set CL_FLAGS=/nologo /c /O1 /Gy /Z7 /EHsc /GS- /Brepro

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
cl %CL_FLAGS% "%SRC%\model.cpp" /Fo:msvc-x64.obj || exit /b 1
cl /nologo /c /O1 /clr /Brepro "%SRC%\clr.cpp" /Fo:msvc-clr.obj || exit /b 1
ml64 /nologo /c /Fo ml64.obj "%SRC%\model64.asm" || exit /b 1
rc /nologo /fo model.res "%SRC%\model.rc" || exit /b 1
cvtres /nologo /machine:x64 /out:res-x64.obj model.res || exit /b 1
del model.res
endlocal

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" x86 >nul 2>&1 || exit /b 1
cl %CL_FLAGS% "%SRC%\model.cpp" /Fo:msvc-x86.obj || exit /b 1
endlocal

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64_arm64 >nul 2>&1 || exit /b 1
cl %CL_FLAGS% /arm64EC "%SRC%\model.cpp" /Fo:msvc-arm64ec.obj || exit /b 1
armasm64 -nologo -g "%SRC%\model-arm64.asm" armasm64.obj || exit /b 1
endlocal

setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
"%LLVM_BIN%\clang-cl" %CL_FLAGS% "%SRC%\model.cpp" /Fo:clang-x64.obj || exit /b 1
"%LLVM_BIN%\clang-cl" %CL_FLAGS% -gdwarf "%SRC%\model.cpp" /Fo:clang-dwarf.obj || exit /b 1
endlocal
"%LLVM_BIN%\llvm-mc" -triple x86_64-pc-windows-msvc -filetype=obj "%SRC%\model.s" -o mc-x64.obj || exit /b 1
