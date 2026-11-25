@echo off
rem Collects the measurements Stackrabbit's work needs and can't take on its own,
rem because they need an administrator. Run it from an elevated command prompt:
rem
rem   collect-data.bat
rem
rem It takes about 5 minutes and writes only to c:\jm\dev-private\stackrabbit, whose
rem contents must never leave this machine.
rem
rem   collected\machine.txt          Defender's state, scans and exclusions; the processes
rem                                  that used the most CPU since boot and in 30 s now.
rem   self-import.etl                A CPU trace (WPR's CPU profile) of Stackrabbit
rem                                  importing s04 and s06, phase by phase.
rem   collected\self-import-bench.txt  What that bench run printed.
rem   corpus\s14-minifilter.etl      A kernel trace with mini-filter events, for the
rem                                  Mini-Filter table. Skipped if it already exists.
rem
rem It cancels any WPR or kernel-logger recording in progress first.
setlocal EnableExtensions
net session >nul 2>&1
if errorlevel 1 (
    echo %~nx0: run this from an elevated ^(administrator^) command prompt 1>&2
    exit /b 1
)

for %%r in ("%~dp0..\..") do set "REPO=%%~fr"
set "SR=%REPO%\target\release\stackrabbit.exe"
set "PRIV=c:\jm\dev-private\stackrabbit"
set "CORPUS=%PRIV%\corpus"
set "OUT=%PRIV%\collected"
set "WORK=%PRIV%\work-minifilter"
set "XPERF=%ProgramFiles(x86)%\Windows Kits\10\Windows Performance Toolkit\xperf.exe"

if not exist "%SR%" (
    echo %~nx0: %SR% is missing; run cargo build --release -p stackrabbit in %REPO% 1>&2
    exit /b 1
)
if not exist "%CORPUS%\s04-general-filemode.etl" (
    echo %~nx0: the corpus is not at %CORPUS% 1>&2
    exit /b 1
)
if not exist "%OUT%" mkdir "%OUT%"

wpr -cancel >nul 2>&1
if exist "%XPERF%" "%XPERF%" -stop >nul 2>&1

rem ---------------------------------------------------------------------
echo [1/3] What keeps the machine busy (about 30 s)
> "%OUT%\machine.txt" echo Collected %DATE% %TIME%
powershell -NoProfile -ExecutionPolicy Bypass -Command "$up = (Get-Date) - (Get-CimInstance Win32_OperatingSystem).LastBootUpTime; 'Up for {0:N1} hours' -f $up.TotalHours; ''; '== Defender =='; Get-MpComputerStatus | Format-List AMRunningMode,RealTimeProtectionEnabled,OnAccessProtectionEnabled,QuickScanEndTime,FullScanEndTime | Out-String -Width 200; '== Defender exclusions (paths) =='; (Get-MpPreference).ExclusionPath; '== Defender exclusions (processes) =='; (Get-MpPreference).ExclusionProcess; ''; '== Defender scans, last 7 days =='; Get-WinEvent -FilterHashtable @{LogName='Microsoft-Windows-Windows Defender/Operational'; Id=1000,1001,1002; StartTime=(Get-Date).AddDays(-7)} -ErrorAction SilentlyContinue | Select-Object -First 40 TimeCreated,Id,@{n='Message';e={($_.Message -split [char]10)[0]}} | Format-Table -AutoSize | Out-String -Width 200; '== Most CPU since boot =='; Get-Process | Where-Object { $_.TotalProcessorTime } | Sort-Object TotalProcessorTime -Descending | Select-Object -First 20 Name,Id,@{n='CpuHours';e={[math]::Round($_.TotalProcessorTime.TotalHours,2)}},@{n='Started';e={$_.StartTime}} | Format-Table -AutoSize | Out-String -Width 200" >> "%OUT%\machine.txt" 2>&1
powershell -NoProfile -ExecutionPolicy Bypass -Command "$a = @{}; Get-Process | ForEach-Object { if ($_.TotalProcessorTime) { $a[$_.Id] = $_.TotalProcessorTime.TotalSeconds } }; $c = 1..30 | ForEach-Object { Start-Sleep -Seconds 1; (Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter \"Name='_Total'\").PercentProcessorTime }; '== Next 30 s =='; 'Total CPU: {0:N1}%%' -f ($c | Measure-Object -Average).Average; Get-Process | Where-Object { $_.TotalProcessorTime } | ForEach-Object { [pscustomobject]@{ Name = $_.Name; Id = $_.Id; CpuSeconds = [math]::Round($_.TotalProcessorTime.TotalSeconds - $a[$_.Id], 2) } } | Sort-Object CpuSeconds -Descending | Select-Object -First 20 | Format-Table -AutoSize | Out-String -Width 200" >> "%OUT%\machine.txt" 2>&1
echo       %OUT%\machine.txt

rem ---------------------------------------------------------------------
echo [2/3] Profiling Stackrabbit's own import of s04 and s06 (about 2 minutes)
wpr -start CPU -filemode
if errorlevel 1 (
    echo %~nx0: wpr could not start; skipped 1>&2
    goto :minifilter
)
"%SR%" bench -phases "%CORPUS%\s04-general-filemode.etl" "%CORPUS%\s06-registry.etl" > "%OUT%\self-import-bench.txt" 2>&1
wpr -stop "%PRIV%\self-import.etl"
echo       %PRIV%\self-import.etl

rem ---------------------------------------------------------------------
:minifilter
echo [3/3] Recording mini-filter events (about 30 s)
if exist "%CORPUS%\s14-minifilter.etl" (
    echo       s14-minifilter.etl already exists; skipped
    goto :done
)
if not exist "%XPERF%" (
    echo       xperf is not installed; skipped
    goto :done
)
if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%"
"%XPERF%" -on PROC_THREAD+LOADER+FILENAME+FLT_IO_INIT+FLT_IO+FLT_FASTIO+FLT_IO_FAILURE -BufferSize 1024 -MinBuffers 64 -f "%WORK%\kernel.etl"
if errorlevel 1 (
    echo %~nx0: xperf could not start; skipped 1>&2
    goto :done
)
rem File work through the file system's filters: create, write, read, rename,
rem list, copy and delete files, plus opening files that don't exist.
powershell -NoProfile -ExecutionPolicy Bypass -Command "$d = '%WORK%\files'; New-Item -ItemType Directory $d | Out-Null; $b = New-Object byte[] 65536; 1..2000 | ForEach-Object { [IO.File]::WriteAllBytes(\"$d\f$_.bin\", $b) }; 1..2000 | ForEach-Object { [void][IO.File]::ReadAllBytes(\"$d\f$_.bin\") }; 1..500 | ForEach-Object { Rename-Item \"$d\f$_.bin\" \"r$_.bin\" }; 1..500 | ForEach-Object { Test-Path \"$d\missing$_.bin\" | Out-Null }; Get-ChildItem -Recurse '%REPO%\crates' | Out-Null; Copy-Item $d \"$d-copy\" -Recurse; Remove-Item $d, \"$d-copy\" -Recurse -Force"
"%XPERF%" -stop
"%XPERF%" -merge "%WORK%\kernel.etl" "%CORPUS%\s14-minifilter.etl"
rmdir /s /q "%WORK%"
for %%f in ("%CORPUS%\s14-minifilter.etl") do echo       %%~ff: %%~zf bytes

:done
echo.
echo Done. Tell Claude it finished; everything stays in %PRIV%.
pause
