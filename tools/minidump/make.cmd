@echo off
rem Builds crash.c for x64, runs it until it crashes and writes its
rem minidump, and keeps the program, its PDB, the dump, and DbgEng's view of
rem the dump (the oracle) in testdata\minidump.
rem
rem Run from a Visual Studio developer prompt (x64), with the WinDbg app's
rem cdb.exe as CDB (or on the PATH). It builds and runs in the repository's
rem target directory, so the paths the files record are the repository's.
setlocal
set here=%~dp0
set out=%here%..\..\testdata\minidump
set work=%here%..\..\target\minidump-fixture
if not exist "%out%" mkdir "%out%"
if not exist "%work%" mkdir "%work%"
pushd "%work%"
cl /nologo /c /O1 /Oy- /GS- /Gy /Zi /MD /Brepro /Fdcrash-x64-c.pdb "%here%crash.c" /Fo:crash-x64.obj >nul || exit /b 1
link /nologo /DEBUG:FULL /OPT:REF /OPT:ICF /INCREMENTAL:NO /Brepro /PDBALTPATH:%%_PDB%% /PDB:crash-x64.pdb crash-x64.obj /OUT:crash-x64.exe >nul || exit /b 1
del crash-x64.dmp 2>nul
rem (Run with the system's PATH only: the stack's old bytes can hold it.)
setlocal
set PATH=%SystemRoot%\system32
.\crash-x64.exe crash-x64.dmp
endlocal
copy /y crash-x64.exe "%out%" >nul
copy /y crash-x64.pdb "%out%" >nul
copy /y crash-x64.dmp "%out%" >nul
if "%CDB%"=="" set CDB=cdb.exe
"%CDB%" -z crash-x64.dmp -y "%work%" -i "%work%" -c ".symopt+0x40;~*k;.ecxr;r;kn;q" > "%out%\crash-x64.cdb.txt" 2>&1
popd
