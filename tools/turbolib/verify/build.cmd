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
set LINK_FLAGS=/nologo /NODEFAULTLIB /ENTRY:entry /SUBSYSTEM:CONSOLE /DEBUG /guard:cf /INCREMENTAL:NO /Brepro main.obj loadcfg.obj kernel32.lib
link %LINK_FLAGS% verify.lib /OUT:link-1.exe /PDB:link-1.pdb || exit /b 1
link %LINK_FLAGS% verify.lib /OUT:link-2.exe /PDB:link-2.pdb || exit /b 1
link %LINK_FLAGS% verify.lib /ORDER:@"%SRC%order.txt" /OUT:link-order.exe /PDB:link-order.pdb || exit /b 1
link %LINK_FLAGS% broken.lib /OUT:link-broken.exe /PDB:link-broken.pdb || exit /b 1
"%LLVM_BIN%\lld-link" %LINK_FLAGS% verify.lib /OUT:lld-1.exe /PDB:lld-1.pdb || exit /b 1
"%LLVM_BIN%\lld-link" %LINK_FLAGS% verify.lib /order:@"%SRC%order.txt" /OUT:lld-order.exe /PDB:lld-order.pdb || exit /b 1
"%LLVM_BIN%\lld-link" %LINK_FLAGS% verify.lib /DEBUG:GHASH /OUT:lld-ghash.exe /PDB:lld-ghash.pdb || exit /b 1
for %%f in (verify.lib broken.lib link-1 link-2 link-order link-broken lld-1 lld-order lld-ghash) do (
  if exist %%f.exe (copy /y %%f.exe "%OUT%" >nul & copy /y %%f.pdb "%OUT%" >nul) else (copy /y %%f "%OUT%" >nul)
)
