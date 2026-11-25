@echo off
rem Builds the archive writer's and the .def reader's test inputs into
rem testdata\coff\archives: libraries made by lib.exe (with and without
rem /Brepro) and llvm-lib of objects that hold every kind of symbol
rem (archives\symbols.py writes them), of ARM64EC, ARM64, and x64 objects
rem (archives\*.c), and import libraries of archives\def\*.def. The tests
rem write each library again and compare.
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set SRC=%~dp0archives
set OUT=%~dp0..\..\testdata\coff\archives
if not defined LLVM_BIN set LLVM_BIN=C:\jm\dev\llvm-project\build-release\bin
set LLVMLIB=%LLVM_BIN%\llvm-lib.exe
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
set WORK=R:\target\fixtures\archives
if exist "%WORK%" rd /s /q "%WORK%"
mkdir "%WORK%" || exit /b 1
cd /d "%WORK%" || exit /b 1
if exist "%OUT%" rd /s /q "%OUT%"
mkdir "%OUT%" || exit /b 1

rem Objects with every kind of symbol, and the libraries of them (each
rem name.lib by lib.exe, name-brepro.lib with /Brepro, name-llvm.lib).
python "%SRC%\symbols.py" || exit /b 1
call :three kinds kinds.obj weak.obj || exit /b 1
call :three common c_def2.obj c_weak.obj c_def.obj c_common.obj || exit /b 1
call :three commons d3.obj d2.obj d1.obj || exit /b 1
call :three sort s1.obj s2.obj s3_with_a_long_name.obj short.obj || exit /b 1
call :three names n23456789.obj n234567890.obj n234567890a.obj n234567890ab.obj n234567890abc.obj sub\x.obj sub2\x.obj sub\long_name_again.obj sub2\long_name_again.obj || exit /b 1
call :three slash sub/fs.obj || exit /b 1
call :three noext noext.obj || exit /b 1

rem ARM64EC, ARM64, and x64 objects: ARM64EC and ARM64X libraries.
set ARMCL="%VCToolsInstallDir%bin\Hostx64\arm64\cl.exe"
%ARMCL% /nologo /c /O1 /GS- /Brepro /arm64EC "%SRC%\ec_a.c" /Fo:a_ec.obj >nul || exit /b 1
%ARMCL% /nologo /c /O1 /GS- /Brepro "%SRC%\ec_a.c" /Fo:a_arm64.obj >nul || exit /b 1
%ARMCL% /nologo /c /O1 /GS- /Brepro /arm64EC "%SRC%\ec_b.c" /Fo:b_ec_with_a_long_name.obj >nul || exit /b 1
%ARMCL% /nologo /c /O1 /GS- /Brepro "%SRC%\ec_b.c" /Fo:b_arm64.obj >nul || exit /b 1
cl /nologo /c /O1 /GS- /Brepro "%SRC%\x64.c" /Fo:c_x64.obj >nul || exit /b 1
call :two ec /machine:arm64ec a_ec.obj b_ec_with_a_long_name.obj || exit /b 1
call :two x /machine:arm64x a_ec.obj a_arm64.obj b_ec_with_a_long_name.obj b_arm64.obj || exit /b 1
call :two mix /machine:arm64ec a_ec.obj b_arm64.obj c_x64.obj || exit /b 1

rem Import libraries of every .def file for x64 (def-<name>.lib); of
rem exports.def for x86, ARM64, and ARM64EC too, and of the others for
rem their machines (def-<name>-<machine>.lib); an ARM64X one; one of x86
rem objects too (-objects: imports UNDECORATE); one without /Brepro
rem (-dated); and llvm-lib's (-llvm; it doesn't take x64.def).
for %%d in ("%SRC%\def\*.def") do (
  lib /nologo /brepro /machine:x64 /def:"%%d" /out:"%OUT%\def-%%~nd.lib" >nul || exit /b 1
)
for %%m in (x86 arm64 arm64ec) do (
  lib /nologo /brepro /machine:%%m /def:"%SRC%\def\exports.def" /out:"%OUT%\def-exports-%%m.lib" >nul || exit /b 1
)
lib /nologo /brepro /machine:x86 /def:"%SRC%\def\x86.def" /out:"%OUT%\def-x86-x86.lib" >nul || exit /b 1
lib /nologo /brepro /machine:arm64ec /def:"%SRC%\def\ec.def" /out:"%OUT%\def-ec-arm64ec.lib" >nul || exit /b 1
lib /nologo /brepro /machine:arm64ec /def:"%SRC%\def\ec2.def" /out:"%OUT%\def-ec2-arm64ec.lib" >nul || exit /b 1
lib /nologo /brepro /machine:arm64x /def:"%SRC%\def\ec.def" /defArm64Native:"%SRC%\def\native.def" /out:"%OUT%\def-ec-arm64x.lib" >nul || exit /b 1
"%VCToolsInstallDir%bin\Hostx64\x86\cl.exe" /nologo /c /O1 /GS- /Brepro "%SRC%\decorated.c" /Fo:decorated.obj >nul || exit /b 1
lib /nologo /brepro /machine:x86 /def:"%SRC%\def\decorated.def" /out:"%OUT%\def-decorated-x86-objects.lib" decorated.obj >nul || exit /b 1
lib /nologo /machine:x64 /def:"%SRC%\def\x64.def" /out:"%OUT%\def-x64-x64-dated.lib" >nul || exit /b 1
"%LLVMLIB%" /machine:x64 /def:"%SRC%\def\x64_llvm.def" /out:"%OUT%\def-x64_llvm-x64-llvm.lib" || exit /b 1
"%LLVMLIB%" /machine:x86 /def:"%SRC%\def\x86.def" /out:"%OUT%\def-x86-x86-llvm.lib" || exit /b 1
"%LLVMLIB%" /machine:arm64ec /def:"%SRC%\def\ec2.def" /out:"%OUT%\def-ec2-arm64ec-llvm.lib" || exit /b 1
"%LLVMLIB%" /machine:arm64x /def:"%SRC%\def\ec.def" /defArm64Native:"%SRC%\def\native.def" /out:"%OUT%\def-ec-arm64x-llvm.lib" || exit /b 1
del "%OUT%\*.exp"
exit /b 0

rem :three name objects...: name.lib (lib.exe), name-brepro.lib, and
rem name-llvm.lib, of the objects in that order on the command line.
:three
set NAME=%1
shift
set OBJS=
:three_args
if "%~1"=="" goto three_go
set OBJS=%OBJS% %1
shift
goto three_args
:three_go
lib /nologo /out:"%OUT%\%NAME%.lib" %OBJS% >nul || exit /b 1
lib /nologo /brepro /out:"%OUT%\%NAME%-brepro.lib" %OBJS% >nul || exit /b 1
"%LLVMLIB%" /out:"%OUT%\%NAME%-llvm.lib" %OBJS% || exit /b 1
exit /b 0

rem :two name /machine:m objects...: name-brepro.lib and name-llvm.lib.
:two
set NAME=%1
set MACHINE=%2
shift
shift
set OBJS=
:two_args
if "%~1"=="" goto two_go
set OBJS=%OBJS% %1
shift
goto two_args
:two_go
lib /nologo /brepro %MACHINE% /out:"%OUT%\%NAME%-brepro.lib" %OBJS% >nul || exit /b 1
"%LLVMLIB%" %MACHINE% /out:"%OUT%\%NAME%-llvm.lib" %OBJS% || exit /b 1
exit /b 0
