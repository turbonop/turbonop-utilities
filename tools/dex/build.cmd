@echo off
rem Builds turbonop-dex's fixtures (testdata\dex) with javac and the Android
rem SDK's d8, and dumps them with its dexdump (the oracle):
rem
rem   Sample-035.dex  (Sample.java) d8 --min-api 21: DEX 035;
rem   Sample-037.dex  --min-api 24: DEX 037 (default and static interface
rem                   methods kept);
rem   Sample-038.dex  --min-api 26: DEX 038 (invoke-polymorphic);
rem   Sample-039.dex  --min-api 28, with Indy (MakeIndy.java, a class file
rem                   written by hand) and Bootstraps.java: DEX 039, call
rem                   sites (invoke-custom), method handles
rem                   (const-method-handle), const-method-type;
rem   Sample-041.dex  the same at --min-api 36 in d8's container format:
rem                   DEX 041;
rem   *.dex.txt       dexdump -a -d -f -h of each (annotations, code,
rem                   headers, class headers).
rem
rem The tools: JAVA_HOME (or the JDK Android Studio installs, or java on
rem PATH), and the newest build-tools and platform of ANDROID_HOME (or
rem ANDROID_SDK_ROOT, %LOCALAPPDATA%\Android\Sdk, or
rem %ProgramFiles(x86)%\Android\android-sdk).
setlocal enabledelayedexpansion
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set OUT=%~dp0..\..\testdata\dex
set WORK=%~dp0..\..\target\fixtures\dex
if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%\classes" || exit /b 1
if not exist "%OUT%" mkdir "%OUT%"

set "SDK=%ANDROID_HOME%"
if "%SDK%"=="" set "SDK=%ANDROID_SDK_ROOT%"
if "%SDK%"=="" if exist "%LOCALAPPDATA%\Android\Sdk\build-tools" set "SDK=%LOCALAPPDATA%\Android\Sdk"
if "%SDK%"=="" if exist "%ProgramFiles(x86)%\Android\android-sdk\build-tools" set "SDK=%ProgramFiles(x86)%\Android\android-sdk"
if "%SDK%"=="" (echo no Android SDK: set ANDROID_HOME 1>&2 & exit /b 1)
set BT=
for /d %%d in ("%SDK%\build-tools\*") do set "BT=%%d"
set LIB=
for /d %%d in ("%SDK%\platforms\android-*") do if exist "%%d\android.jar" set "LIB=%%d\android.jar"
if "%BT%"=="" (echo no build-tools in !SDK! 1>&2 & exit /b 1)
if "%LIB%"=="" (echo no platform in !SDK! 1>&2 & exit /b 1)

if "%JAVA_HOME%"=="" for /d %%d in ("%ProgramFiles%\Android\openjdk\jdk-*") do set "JAVA_HOME=%%d"
if not "%JAVA_HOME%"=="" set "PATH=%JAVA_HOME%\bin;%PATH%"
where javac >nul 2>nul || (echo no JDK: set JAVA_HOME 1>&2 & exit /b 1)

cd /d "%WORK%" || exit /b 1
javac -g -parameters --release 11 -encoding US-ASCII -Werror -Xlint:-options -d classes ^
    "%~dp0Sample.java" "%~dp0Bootstraps.java" || exit /b 1
java "%~dp0MakeIndy.java" classes || exit /b 1

set D8=java -cp "%BT%\lib\d8.jar" com.android.tools.r8.D8 --debug --lib "%LIB%"
set SAMPLE=
for %%f in (classes\dex\sample\*.class) do (
    set "NAME=%%~nf"
    if not "!NAME!"=="Indy" if not "!NAME!"=="Bootstraps" set "SAMPLE=!SAMPLE! %%f"
)
set ALL=%SAMPLE% classes\dex\sample\Indy.class classes\dex\sample\Bootstraps.class

call :d8 035 "--min-api 21" "%SAMPLE%" || exit /b 1
call :d8 037 "--min-api 24" "%SAMPLE%" || exit /b 1
call :d8 038 "--min-api 26" "%SAMPLE%" || exit /b 1
call :d8 039 "--min-api 28" "%ALL%" || exit /b 1
set "D8=java -Dcom.android.tools.r8.dexContainerExperiment=true -cp "%BT%\lib\d8.jar" com.android.tools.r8.D8 --debug --lib "%LIB%""
call :d8 041 "--min-api 36" "%ALL%" || exit /b 1
exit /b 0

rem :d8 <version> <options> <classes>: Sample-<version>.dex and its dump.
:d8
mkdir out%1 || exit /b 1
%D8% %~2 --output out%1 %~3 2>d8-%1.log || (type d8-%1.log 1>&2 & exit /b 1)
copy /y out%1\classes.dex Sample-%1.dex >nul || exit /b 1
"%BT%\dexdump.exe" -a -d -f -h Sample-%1.dex > Sample-%1.dex.txt 2>dexdump-%1.log || (type dexdump-%1.log 1>&2 & exit /b 1)
copy /y Sample-%1.dex "%OUT%\Sample-%1.dex" >nul || exit /b 1
copy /y Sample-%1.dex.txt "%OUT%\Sample-%1.dex.txt" >nul || exit /b 1
exit /b 0
