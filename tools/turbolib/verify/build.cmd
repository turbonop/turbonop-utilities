@echo off
rem Builds turbolib verify's and compare-images's test inputs into
rem testdata\turbolib, from the sources here:
rem
rem   verify.lib         a.obj, b.obj, c.obj (cl /O1 /Gy /Z7: the COMDATs
rem                      of shared.h in each), gl.obj (/GL), and the import
rem                      objects of imports.def, archived by lib.exe
rem   link-1.exe/.pdb    main.obj, loadcfg.obj, and verify.lib, linked by link.exe
rem   link-2.exe/.pdb    the same link again (link.exe's PDBs differ)
rem   link-order.*       the same, with order.txt as /ORDER: other addresses
rem   link-broken.*      against broken.lib, whose shared_add has other bytes
rem   lld-1.*            main.obj and verify.lib, linked by lld-link
rem   lld-order.*        the same, with order.txt as /order
rem   lld-ghash.*        lld-link with /DEBUG:GHASH
rem
rem Every program is /guard:cf, with no C runtime (see main.cpp). LLVM_BIN
rem is where lld-link is (the default is a Release build of llvm-project at
rem C:\jm\dev\llvm-project\build-release).
setlocal
rem (Run from R:, the repository mapped: see ..\..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\..\neutral.cmd" "%~f0" & exit /b)
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set SRC=%~dp0
set OUT=%~dp0..\..\..\testdata\turbolib
set WORK=%~dp0..\..\..\target\fixtures\turbolib-verify
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
if not exist "%OUT%" mkdir "%OUT%" || exit /b 1
if not exist "%WORK%\broken" mkdir "%WORK%\broken" || exit /b 1
cd /d "%WORK%" || exit /b 1
set CL_FLAGS=/nologo /c /O1 /Gy /Z7 /GS- /guard:cf /Zc:threadSafeInit- /GR- /Brepro
cl %CL_FLAGS% "%SRC%a.cpp" "%SRC%b.cpp" "%SRC%c.cpp" "%SRC%main.cpp" "%SRC%loadcfg.c" || exit /b 1
cl %CL_FLAGS% /DBROKEN "%SRC%a.cpp" "%SRC%b.cpp" "%SRC%c.cpp" /Fo:broken\ || exit /b 1
cl /nologo /c /O1 /GL /GS- /Brepro "%SRC%gl.cpp" || exit /b 1
lib /nologo /Brepro /machine:x64 /def:"%SRC%imports.def" /out:imports.lib || exit /b 1
lib /nologo /Brepro /out:verify.lib a.obj b.obj c.obj gl.obj imports.lib || exit /b 1
lib /nologo /Brepro /out:broken.lib broken\a.obj broken\b.obj broken\c.obj gl.obj imports.lib || exit /b 1
rem Every program is linked as prog.exe and prog.pdb, in this directory, and
rem copied out: the PDBs record their paths and command lines, which are then
rem the same in each.
set LINK_FLAGS=/nologo /NODEFAULTLIB /ENTRY:entry /SUBSYSTEM:CONSOLE /DEBUG /guard:cf /INCREMENTAL:NO /Brepro main.obj loadcfg.obj kernel32.lib /OUT:prog.exe /PDB:prog.pdb
set LLD="%LLVM_BIN%\lld-link"
call :link link-1 link %LINK_FLAGS% verify.lib || exit /b 1
call :link link-2 link %LINK_FLAGS% verify.lib || exit /b 1
call :link link-order link %LINK_FLAGS% verify.lib /ORDER:@"%SRC%order.txt" || exit /b 1
call :link link-broken link %LINK_FLAGS% broken.lib || exit /b 1
call :link lld-1 %LLD% %LINK_FLAGS% verify.lib || exit /b 1
call :link lld-order %LLD% %LINK_FLAGS% verify.lib /order:@"%SRC%order.txt" || exit /b 1
call :link lld-ghash %LLD% %LINK_FLAGS% verify.lib /DEBUG:GHASH || exit /b 1
copy /y verify.lib "%OUT%" >nul || exit /b 1
copy /y broken.lib "%OUT%" >nul || exit /b 1
exit /b 0

rem :link <name> <linker> <arguments>: links, and copies the program out as
rem <name>.exe and <name>.pdb.
:link
set NAME=%1
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
copy /y prog.exe "%OUT%\%NAME%.exe" >nul || exit /b 1
copy /y prog.pdb "%OUT%\%NAME%.pdb" >nul || exit /b 1
exit /b 0
