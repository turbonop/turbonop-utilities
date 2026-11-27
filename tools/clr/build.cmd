@echo off
rem Builds turbonop-clr's fixtures (testdata\clr) with the .NET SDK's C#
rem compiler, against the .NET 8 reference assemblies, deterministically:
rem
rem   Sample.dll            (Sample.cs) a library with a little of everything,
rem                         public-signed (sample.pub), with Sample.Extra.netmodule
rem                         added, two embedded resources (a .resources file that
rem                         ResGen.cs writes, and notes.txt) and a linked one;
rem   Sample.pdb            its portable PDB;
rem   Sample.Extra.netmodule (Extra.cs) a module;
rem   Hello.exe             (Hello.cs) a 32-bit program;
rem   oracle\ClrDump.dll    (ClrDump.cs) the oracle, System.Reflection.Metadata's
rem                         view of a file, which the tests run on the machine's
rem                         framework assemblies when there is a dotnet to run it;
rem   *.dump.txt            the oracle's output for each fixture, and
rem   opcodes.txt           its CIL opcode table (System.Reflection.Emit's).
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set OUT=%~dp0..\..\testdata\clr
set WORK=%~dp0..\..\target\fixtures\clr
if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%" || exit /b 1
if not exist "%OUT%\oracle" mkdir "%OUT%\oracle"

rem The SDK (its compiler) and the reference assemblies.
for %%i in (dotnet.exe) do set "DOTNET=%%~dp$PATH:i"
if "%DOTNET%"=="" (echo dotnet is not on PATH 1>&2 & exit /b 1)
for /f "tokens=1" %%v in ('dotnet --list-sdks') do set SDK=%%v
set "CSC=%DOTNET%sdk\%SDK%\Roslyn\bincore\csc.dll"
if not exist "%CSC%" (echo no compiler at %CSC% 1>&2 & exit /b 1)
set REFS=
for /d %%d in ("%DOTNET%packs\Microsoft.NETCore.App.Ref\8.0.*") do set "REFS=%%d\ref\net8.0"
if "%REFS%"=="" (echo no .NET 8 reference assemblies 1>&2 & exit /b 1)

cd /d "%WORK%" || exit /b 1
(for %%f in ("%REFS%\*.dll") do @echo /reference:"%%f") > refs.rsp
set CS=dotnet "%CSC%" /nologo /noconfig /nostdlib+ /deterministic /optimize+ /langversion:latest /warnaserror+ @refs.rsp
set RUNTIMECONFIG={"runtimeOptions":{"tfm":"net8.0","rollForward":"Major","framework":{"name":"Microsoft.NETCore.App","version":"8.0.0"}}}

%CS% /target:exe /debug- /out:ResGen.dll "%~dp0ResGen.cs" || exit /b 1
echo %RUNTIMECONFIG%> ResGen.runtimeconfig.json
dotnet ResGen.dll Sample.Strings.resources || exit /b 1

%CS% /target:module /debug- /out:Sample.Extra.netmodule "%~dp0Extra.cs" || exit /b 1
copy /y "%~dp0notes.txt" notes.txt >nul || exit /b 1
copy /y "%~dp0linked.txt" linked.txt >nul || exit /b 1
%CS% /target:library /unsafe+ /debug:portable /pdb:Sample.pdb /out:Sample.dll ^
    /keyfile:"%~dp0sample.pub" /publicsign+ /addmodule:Sample.Extra.netmodule ^
    /resource:Sample.Strings.resources /resource:notes.txt,Sample.notes.txt,private ^
    /linkresource:linked.txt,Sample.linked.txt "%~dp0Sample.cs" || exit /b 1
%CS% /target:exe /platform:x86 /debug- /out:Hello.exe "%~dp0Hello.cs" || exit /b 1

%CS% /target:exe /debug- /out:ClrDump.dll "%~dp0ClrDump.cs" || exit /b 1
echo %RUNTIMECONFIG%> ClrDump.runtimeconfig.json

for %%f in (Sample.dll Sample.pdb Sample.Extra.netmodule Hello.exe) do (
    copy /y %%f "%OUT%\%%f" >nul || exit /b 1
    dotnet ClrDump.dll %%f > "%OUT%\%%f.dump.txt" || exit /b 1
)
dotnet ClrDump.dll -opcodes > "%OUT%\opcodes.txt" || exit /b 1
copy /y ClrDump.dll "%OUT%\oracle\ClrDump.dll" >nul || exit /b 1
copy /y ClrDump.runtimeconfig.json "%OUT%\oracle\ClrDump.runtimeconfig.json" >nul || exit /b 1
exit /b 0
