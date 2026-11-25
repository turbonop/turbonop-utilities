@echo off
rem Builds crash.c for x64 (or x86: make.cmd x86, from an x86 developer
rem prompt), runs it until it crashes and writes its minidump, and keeps the
rem program, its PDB, the dump, and DbgEng's view of the dump (the oracle) in
rem testdata\minidump.
rem
rem Run from a Visual Studio developer prompt for the architecture, with the
rem WinDbg app's cdb.exe as CDB (or on the PATH). It builds and runs in the
rem repository's target directory, so the paths the files record are the
rem repository's. The x86 build omits frame pointers (/Oy), for FPO and
rem frame data.
setlocal
set arch=%1
if "%arch%"=="" set arch=x64
set here=%~dp0
set out=%here%..\..\testdata\minidump
set work=%here%..\..\target\minidump-fixture-%arch%
set name=crash-%arch%
set fp=/Oy-
if "%arch%"=="x86" set fp=/Oy
if not exist "%out%" mkdir "%out%"
if not exist "%work%" mkdir "%work%"
pushd "%work%"
cl /nologo /c /O1 %fp% /GS- /Gy /Zi /MD /Brepro /Fd%name%-c.pdb "%here%crash.c" /Fo:%name%.obj >nul || exit /b 1
link /nologo /DEBUG:FULL /OPT:REF /OPT:ICF /INCREMENTAL:NO /Brepro /PDBALTPATH:%%_PDB%% /PDB:%name%.pdb %name%.obj /OUT:%name%.exe >nul || exit /b 1
del %name%.dmp 2>nul
rem (Run with the system's PATH only: the stack's old bytes can hold it.)
setlocal
set PATH=%SystemRoot%\system32
.\%name%.exe %name%.dmp
endlocal
copy /y %name%.exe "%out%" >nul
copy /y %name%.pdb "%out%" >nul
copy /y %name%.dmp "%out%" >nul
if "%CDB%"=="" set CDB=cdb.exe
"%CDB%" -z %name%.dmp -y "%work%" -i "%work%" -c ".symopt+0x40;~*k;.ecxr;r;kn;.frame 1;dv /t /v;q" > "%out%\%name%.cdb.txt" 2>&1
popd
