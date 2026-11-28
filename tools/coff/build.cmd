@echo off
rem Builds Hexcavator's COFF test inputs into testdata\coff: objects.obj (/Z7,
rem /Gy), objects-big.obj (/bigobj), objects-zi.obj (/Zi: its types in
rem objects-zi.pdb, a type server), objects.lib (objects.obj and
rem objects-big.obj), imports.lib (an import library from imports.def), and
rem LTCG (/GL) objects: ltcg.obj (x64, from ltcg.cpp; and ltcg.lib of it) and
rem ltcg-x86.obj (x86, from ltcg.c), and ltcg-refs.obj (x64, from ltcg-refs.cpp).
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set OUT=%~dp0..\..\testdata\coff
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath`) do set VS=%%i
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" amd64 >nul 2>&1 || exit /b 1
cd /d "%OUT%" || exit /b 1
cl /nologo /c /O1 /Gy /Z7 /GS- /Brepro "%~dp0objects.cpp" /Fo:objects.obj || exit /b 1
cl /nologo /c /O1 /Gy /Z7 /GS- /Brepro /bigobj "%~dp0objects.cpp" /Fo:objects-big.obj || exit /b 1
cl /nologo /c /O1 /Gy /Zi /GS- /Brepro /Fdobjects-zi.pdb "%~dp0objects.cpp" /Fo:objects-zi.obj || exit /b 1
lib /nologo /Brepro /out:objects.lib objects.obj objects-big.obj || exit /b 1
lib /nologo /Brepro /machine:x64 /def:"%~dp0imports.def" /out:imports.lib || exit /b 1
del imports.exp
rem The tools' views of the libraries, which the archive tests compare with:
rem the members (lib /list) and the symbol index (dumpbin /linkermember:2).
for %%l in (objects imports) do (
  lib /nologo /list %%l.lib > %%l.lib.list.txt || exit /b 1
  dumpbin /nologo /linkermember:2 %%l.lib > %%l.lib.linkermember.txt || exit /b 1
)
rem dumpbin's view of each object's symbol table (hexcavator dump -symbols matches).
for %%o in (objects.obj objects-big.obj) do (
  dumpbin /nologo /symbols %%o > %%o.symbols.txt || exit /b 1
)
rem LTCG objects, and the tools' views of them: dumpbin's /headers (which
rem hexcavator dump matches), lib's symbol index (which the symbols hexcavator ltcg finds
rem as public definitions hold), and the directives of the same source
rem compiled without /GL.
cl /nologo /c /O1 /GL /GS- /Brepro "%~dp0ltcg.cpp" /Fo:ltcg.obj || exit /b 1
cl /nologo /c /O1 /GS- /Brepro "%~dp0ltcg.cpp" /Fo:ltcg-nogl.obj || exit /b 1
dumpbin /nologo /directives ltcg-nogl.obj > ltcg.directives.txt || exit /b 1
del ltcg-nogl.obj
lib /nologo /Brepro /out:ltcg.lib ltcg.obj || exit /b 1
dumpbin /nologo /headers ltcg.obj > ltcg.obj.headers.txt || exit /b 1
dumpbin /nologo /linkermember:2 ltcg.lib > ltcg.lib.linkermember.txt || exit /b 1
rem /GL IL references whose fields take more than a byte (ltcg-refs.cpp):
rem lib.exe's index of them.
cl /nologo /c /Od /Ob0 /GL /GS- /Brepro "%~dp0ltcg-refs.cpp" /Fo:ltcg-refs.obj || exit /b 1
lib /nologo /Brepro /out:ltcg-refs.lib ltcg-refs.obj || exit /b 1
dumpbin /nologo /linkermember:2 ltcg-refs.lib > ltcg-refs.lib.linkermember.txt || exit /b 1
del ltcg-refs.lib
setlocal
call "%VS%\VC\Auxiliary\Build\vcvarsall.bat" x86 >nul 2>&1 || exit /b 1
cl /nologo /c /O1 /GL /GS- /Brepro "%~dp0ltcg.c" /Fo:ltcg-x86.obj || exit /b 1
lib /nologo /Brepro /out:ltcg-x86.lib ltcg-x86.obj || exit /b 1
dumpbin /nologo /headers ltcg-x86.obj > ltcg-x86.obj.headers.txt || exit /b 1
dumpbin /nologo /linkermember:2 ltcg-x86.lib > ltcg-x86.lib.linkermember.txt || exit /b 1
del ltcg-x86.lib
endlocal
