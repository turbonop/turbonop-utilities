@echo off
rem Builds the LTCG test programs here with the installed compiler, then
rem links each with the link.exe in <toolchain> (a copy of a toolset's
rem bin\Hostx64\x64 with the probe installed as c2.dll: see
rem crates/link/ltcg-probe), each link's probe log beside its output:
rem
rem   run.cmd <toolchain> <out>
rem
rem The outputs and logs are this machine's: keep them out of the repository.
setlocal
if "%~2"=="" (echo usage: run.cmd ^<toolchain^> ^<out^> & exit /b 2)
set "TC=%~f1"
set "OUT=%~f2"
set "SRC=%~dp0"
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -prerelease -products * -property installationPath`) do set "VS=%%i"
call "%VS%\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
if not exist "%OUT%" mkdir "%OUT%"
cd /d "%OUT%" || exit /b 1

set "CL_GL=/nologo /c /O2 /GL /Zi /MD"

rem One translation unit.
cl %CL_GL% "%SRC%one.c" /Foone.obj /Fdone-c.pdb || exit /b 1
call :link one /OUT:one.exe one.obj || exit /b 1

rem Several translation units.
cl %CL_GL% "%SRC%multi1.c" "%SRC%multi2.c" /Fdmulti-c.pdb || exit /b 1
call :link multi /OUT:multi.exe multi1.obj multi2.obj || exit /b 1

rem /GL and native objects together.
cl %CL_GL% "%SRC%mixed_gl.c" /Fdmixed-c.pdb || exit /b 1
cl /nologo /c /O2 /Zi /MD "%SRC%mixed_native.c" /Fdmixed-c.pdb || exit /b 1
call :link mixed /OUT:mixed.exe mixed_gl.obj mixed_native.obj || exit /b 1

rem A library of /GL objects.
cl %CL_GL% "%SRC%lib1.c" "%SRC%lib2.c" "%SRC%libmain.c" /Fdlib-c.pdb || exit /b 1
lib /nologo /LTCG /OUT:gl.lib lib1.obj lib2.obj || exit /b 1
call :link lib /OUT:lib.exe libmain.obj gl.lib || exit /b 1

rem A DLL: C++, exceptions, exports, full debug information.
cl %CL_GL% /EHsc /std:c++17 "%SRC%dll.cpp" /Fddll-c.pdb || exit /b 1
call :link dll /DLL /OUT:probe.dll dll.obj || exit /b 1

rem Incremental LTCG: linked twice, the second reusing the first's .iobj.
call :linkinc inc1 /OUT:inc.exe multi1.obj multi2.obj || exit /b 1
call :linkinc inc2 /OUT:inc.exe multi1.obj multi2.obj || exit /b 1

echo done: %OUT%
exit /b 0

:linkinc
set "NAME=%~1"
shift
set "C2PROBE_LOG=%OUT%\%NAME%.probe.log"
"%TC%\link.exe" /nologo /LTCG:INCREMENTAL /DEBUG:FULL /PDB:inc.pdb %1 %2 %3 %4 %5 %6 %7 %8 %9 > "%NAME%.link.txt" 2>&1
set "RC=%ERRORLEVEL%"
type "%NAME%.link.txt"
echo %NAME%: link exit %RC%
exit /b %RC%

:link
set "NAME=%~1"
shift
set "C2PROBE_LOG=%OUT%\%NAME%.probe.log"
"%TC%\link.exe" /nologo /LTCG /DEBUG:FULL /PDB:%NAME%.pdb %1 %2 %3 %4 %5 %6 %7 %8 %9 > "%NAME%.link.txt" 2>&1
set "RC=%ERRORLEVEL%"
type "%NAME%.link.txt"
echo %NAME%: link exit %RC%
exit /b %RC%
