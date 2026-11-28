@echo off
rem Builds turbolib's tests of ARM64X libraries archived again into
rem testdata\coff\arm64x, from the .def files in arm64x\: import libraries
rem lib /DEF makes (x.lib, ARM64X: ec.def's ARM64EC half and native.def's
rem ARM64 one; ec.lib, ARM64EC; c.lib, ARM64X with constants; x64.lib,
rem x64b.lib, x64; nat.lib, ARM64), and what lib.exe 14.52 makes of them
rem archived again (lib-*.lib; see the commands below), which turbolib's
rem tests compare with byte for byte.
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set SRC=%~dp0arm64x
set OUT=%~dp0..\..\testdata\coff\arm64x
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
if not exist "%OUT%" mkdir "%OUT%" || exit /b 1
cd /d "%OUT%" || exit /b 1
set L=lib /nologo /Brepro
%L% /machine:arm64x /def:"%SRC%\ec.def" /defArm64Native:"%SRC%\native.def" /out:x.lib >nul || exit /b 1
%L% /machine:arm64ec /def:"%SRC%\ec.def" /out:ec.lib >nul || exit /b 1
%L% /machine:arm64x /def:"%SRC%\ecc.def" /defArm64Native:"%SRC%\natc.def" /out:c.lib >nul || exit /b 1
%L% /machine:x64 /def:"%SRC%\x64.def" /out:x64.lib >nul || exit /b 1
%L% /machine:x64 /def:"%SRC%\x64b.def" /out:x64b.lib >nul || exit /b 1
%L% /machine:arm64 /def:"%SRC%\nat.def" /out:nat.lib >nul || exit /b 1
del *.exp
rem name, then lib.exe's arguments (each case's output is lib-<name>.lib).
call :one nomach /REMOVE:none.obj x.lib
call :one arm64x /machine:arm64x /REMOVE:none.obj x.lib
call :one arm64 /machine:arm64 /REMOVE:none.obj x.lib
call :one arm64ec /machine:arm64ec /REMOVE:none.obj x.lib
call :one ec-nomach /REMOVE:none.obj ec.lib
call :one ec-arm64 /machine:arm64 /REMOVE:none.obj ec.lib
call :one ec-arm64ec /machine:arm64ec /REMOVE:none.obj ec.lib
call :one x-x64-nomach x.lib x64.lib
call :one ec-x64-nomach ec.lib x64.lib
call :one nat-x64-nomach nat.lib x64.lib
call :one const-arm64x /machine:arm64x /REMOVE:none.obj c.lib
call :one x64-dup x64.lib x64b.lib
exit /b 0

:one
set NAME=%1
shift
set ARGS=
:args
if "%~1"=="" goto run
set ARGS=%ARGS% %1
shift
goto args
:run
%L% /out:lib-%NAME%.lib %ARGS% > lib-%NAME%.txt 2>&1
exit /b 0
