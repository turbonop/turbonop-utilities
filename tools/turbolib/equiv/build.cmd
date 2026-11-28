@echo off
rem Builds compare-images's Debug equivalence fixtures into
rem testdata\turbolib\equiv, from the sources here:
rem
rem   a.obj, b.obj, c.obj  cl /Od /Z7 /RTC1 /EHsc /MDd: each holding copies of
rem                        shared.h's COMDATs (switch tables, inline
rem                        thread_locals, a lambda, exceptions, RTTI, two
rem                        functions of the same code)
rem   original.lib         them, archived by lib.exe
rem   folded.lib           them, archived by turbolib --fold=identical
rem   link-original.*      main.obj and original.lib, linked by link.exe
rem   link-folded.*        main.obj and folded.lib, linked by link.exe
rem   link-broken.*        main.obj and folded.lib with one of classify's
rem                        constants changed (breakfold.py), by link.exe
rem   link-retarget.*      ... with a relocation of shared_all naming twin_a
rem                        for twin_b, the same code
rem   lld-original.*       the same as link-original, with lld-link
rem   lld-folded.*
rem
rem compare-images finds each pair equivalent; its tests change the folded
rem programs to show that it catches real differences. TURBOLIB is
rem turbolib.exe (the default: this repository's Release build); LLVM_BIN is
rem where lld-link is (the default is a Release build of llvm-project at
rem C:\jm\dev\llvm-project\build-release).
setlocal
rem (Run from R:, the repository mapped: see ..\..\neutral.cmd; with
rem EQUIV_IN_PLACE set, where it is, when another build holds R:.)
if not defined EQUIV_IN_PLACE if /i not "%~d0"=="R:" (call "%~dp0..\..\neutral.cmd" "%~f0" & exit /b)
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
if not defined TURBOLIB set TURBOLIB=%~dp0..\..\..\target\release\turbolib.exe
set SRC=%~dp0
set OUT=%~dp0..\..\..\testdata\turbolib\equiv
set WORK=%~dp0..\..\..\target\fixtures\turbolib-equiv
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
if not exist "%OUT%" mkdir "%OUT%" || exit /b 1
if not exist "%WORK%" mkdir "%WORK%" || exit /b 1
cd /d "%WORK%" || exit /b 1
rem turbolib's lib personality: a copy named lib.exe.
if not exist fold mkdir fold || exit /b 1
copy /y "%TURBOLIB%" fold\lib.exe >nul || exit /b 1
set CL_FLAGS=/nologo /c /std:c++17 /Od /Z7 /RTC1 /EHsc /MDd /Brepro
cl %CL_FLAGS% "%SRC%a.cpp" "%SRC%b.cpp" "%SRC%c.cpp" "%SRC%main.cpp" || exit /b 1
lib /nologo /Brepro /out:original.lib a.obj b.obj c.obj || exit /b 1
set TURBOLIB_OPTIONS=--fold=identical --strict
fold\lib.exe /nologo /Brepro /out:folded.lib a.obj b.obj c.obj || exit /b 1
set TURBOLIB_OPTIONS=
"%TURBOLIB%" verify original.lib folded.lib || exit /b 1
rem The folded library broken as a fault in folding could: classify's
rem "case 4: return 53" returning 54; shared_all's call of twin_b sent to
rem twin_a (the same code).
python "%SRC%breakfold.py" folded.lib broken.lib bytes ?classify@@YAHH@Z "b8 35 00 00 00" "b8 36 00 00 00" || exit /b 1
python "%SRC%breakfold.py" folded.lib retarget.lib retarget ?shared_all@@YAHH@Z ?twin_b@@YAHH@Z ?twin_a@@YAHH@Z || exit /b 1
rem Every program is linked as prog.exe and prog.pdb against lib.lib, in
rem this directory, and copied out: the PDBs record their paths and command
rem lines, which are then the same in each.
set LINK_FLAGS=/nologo /DEBUG /INCREMENTAL:NO /Brepro main.obj lib.lib /OUT:prog.exe /PDB:prog.pdb
set LLD="%LLVM_BIN%\lld-link"
call :link link-original original.lib link %LINK_FLAGS% || exit /b 1
call :link link-folded folded.lib link %LINK_FLAGS% || exit /b 1
call :link link-broken broken.lib link %LINK_FLAGS% || exit /b 1
call :link link-retarget retarget.lib link %LINK_FLAGS% || exit /b 1
call :link lld-original original.lib %LLD% %LINK_FLAGS% || exit /b 1
call :link lld-folded folded.lib %LLD% %LINK_FLAGS% || exit /b 1
exit /b 0

rem :link <name> <library> <linker> <arguments>: links against <library> as
rem lib.lib, runs the program, and copies it out as <name>.exe and
rem <name>.pdb.
:link
set NAME=%1
copy /y %2 lib.lib >nul || exit /b 1
shift
shift
set ARGS=
:args
if "%~1"=="" goto run
set ARGS=%ARGS% %1
shift
goto args
:run
if exist prog.pdb del prog.pdb
if exist prog.exe del prog.exe
%ARGS% || exit /b 1
.\prog.exe || exit /b 1
copy /y prog.exe "%OUT%\%NAME%.exe" >nul || exit /b 1
copy /y prog.pdb "%OUT%\%NAME%.pdb" >nul || exit /b 1
exit /b 0
