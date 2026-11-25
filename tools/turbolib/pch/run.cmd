@echo off
rem Every precompiled-types spike in order: build, synthesize, link, check.
cd /d %~dp0
call "%~dp0build.cmd" || exit /b 1
python -I spike.py full a.obj || exit /b 1
python -I spike.py share a.obj b.obj || exit /b 1
call "%~dp0link.cmd"
rem Function ids in a PCH: the prefix counts are for a.obj as VS 18 built it
rem (16 records stop before its LF_FUNC_IDs, 22 include them). Check them
rem against llvm-pdbutil dump -types a.obj with another compiler.
python -I spike.py prefix a.obj 16 noids || exit /b 1
python -I spike.py prefix a.obj 22 ids || exit /b 1
call "%~dp0link2.cmd"
python -I chks64.py || exit /b 1
call "%~dp0chk.cmd"
