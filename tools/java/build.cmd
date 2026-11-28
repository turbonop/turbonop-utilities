@echo off
rem Builds turbonop-java's fixtures (testdata\java) with a JDK (JAVA_HOME's,
rem or the javac on PATH; JDK 21, whose javap the expected text is from):
rem
rem   main\...\*.class        src\ (module-info.java and package fixture) for
rem                           Java 21, with every debug attribute (-g) and
rem                           parameter names (-parameters);
rem   old\                    old\ for Java 8;
rem   preview\                preview\ for Java 21 with preview features;
rem   mr\base, mr\17          mr\base for Java 8 and mr\17 for Java 17;
rem   fixtures.jar            the classes of main\, a modular JAR (the jar tool
rem                           adds ModulePackages and ModuleMainClass to its
rem                           module-info.class), with a main class, a
rem                           manifest of main and per-entry attributes, and
rem                           signed (a throwaway EC key, made each build);
rem   mr.jar                  a multi-release JAR: mr\base and old\, and
rem                           mr\17 under META-INF/versions/17;
rem   *.class.javap.txt       javap -v -p of each class (and of the JAR's
rem                           module-info.class), UTF-8, which
rem                           crates/formats/java/tests/oracle.rs reproduces.
setlocal
rem (Run from R:, the repository mapped: see ..\neutral.cmd.)
if /i not "%~d0"=="R:" (call "%~dp0..\neutral.cmd" "%~f0" & exit /b)
set OUT=%~dp0..\..\testdata\java
set WORK=%~dp0..\..\target\fixtures\java
if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%" || exit /b 1
for %%d in (main old preview mr) do if exist "%OUT%\%%d" rmdir /s /q "%OUT%\%%d"
if not exist "%OUT%" mkdir "%OUT%"

if defined JAVA_HOME (set "JBIN=%JAVA_HOME%\bin\") else (for %%i in (javac.exe) do set "JBIN=%%~dp$PATH:i")
if not exist "%JBIN%javac.exe" (echo no JDK: set JAVA_HOME or put javac on PATH 1>&2 & exit /b 1)
set JAVAC="%JBIN%javac.exe" -encoding UTF-8 -g -parameters -Xlint:none -nowarn
set DATE=--date=2026-01-01T00:00:00Z

cd /d "%~dp0" || exit /b 1
set SRC=
for /r src %%f in (*.java) do call set "SRC=%%SRC%% "%%f""
%JAVAC% --release 21 -d "%WORK%\main" %SRC% || exit /b 1
%JAVAC% --release 8 -d "%WORK%\old" old\legacy\Legacy.java || exit /b 1
%JAVAC% --release 21 --enable-preview -d "%WORK%\preview" preview\preview\Preview.java || exit /b 1
%JAVAC% --release 8 -d "%WORK%\mr\base" mr\base\mr\Version.java || exit /b 1
%JAVAC% --release 17 -d "%WORK%\mr\17" mr\17\mr\Version.java || exit /b 1

cd /d "%WORK%" || exit /b 1
rem The manifest's other attributes: a value longer than a line (continued),
rem and a section for a package.
(
echo Implementation-Title: turbonop-java's fixtures
echo Implementation-Version: 1.2.3
echo X-Long-Attribute: a value long enough that the jar tool continues it on the next line, and the line after that, as manifests wrap at 72 bytes
echo.
echo Name: fixture/internal/
echo Sealed: true
echo Implementation-Title: the internal package
echo.
) > extra.mf
"%JBIN%jar.exe" --create --file fixtures.jar %DATE% --main-class fixture.Hello --manifest extra.mf -C main . || exit /b 1
"%JBIN%keytool.exe" -genkeypair -alias fixture -keyalg EC -groupname secp256r1 -dname CN=fixture ^
    -validity 36500 -keystore keys.p12 -storetype PKCS12 -storepass fixture -keypass fixture >nul 2>nul || exit /b 1
"%JBIN%jarsigner.exe" -keystore keys.p12 -storepass fixture fixtures.jar fixture >nul || exit /b 1
"%JBIN%jar.exe" --create --file mr.jar %DATE% -C mr\base . -C old . --release 17 -C mr\17 . || exit /b 1

rem Every class, and its javap -v -p.
set JAVAP="%JBIN%javap.exe" -J-Dstdout.encoding=UTF-8 -J-Duser.language=en -v -p
for %%d in (main old preview mr) do xcopy /s /q /y %%d "%OUT%\%%d\" >nul || exit /b 1
cd /d "%OUT%" || exit /b 1
for /r %%f in (*.class) do (
    %JAVAP% "%%f" > "%%f.javap.txt" || exit /b 1
)
copy /y "%WORK%\fixtures.jar" "%OUT%\fixtures.jar" >nul || exit /b 1
copy /y "%WORK%\mr.jar" "%OUT%\mr.jar" >nul || exit /b 1
rem (By URL: given a class path, javap finds the JDK's own module-info first.)
%JAVAP% "jar:file:///R:/testdata/java/fixtures.jar!/module-info.class" > "%OUT%\fixtures.jar.module-info.javap.txt" || exit /b 1
exit /b 0
