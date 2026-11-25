@echo off
cd /d %~dp0
call "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Auxiliary\Build\vcvars64.bat" >nul 2>nul
copy /y b.obj c.obj >nul
lib /nologo /OUT:abc.lib a.obj b.obj x.obj c.obj 2>&1
lib /nologo /OUT:ba.lib b.obj a.obj
dumpbin /nologo /linkermember orig.lib | findstr /i "shared_f offset member"
echo ---- ba.lib
dumpbin /nologo /linkermember ba.lib | findstr /i "shared_f"
