@echo off
rem Runs a command in the Visual Studio x64 environment: vsenv.cmd <command> [args...]
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
%*
