@echo off
rem nukeplant one-click launcher.
rem Usage: nukeplant.bat [rebake^|bake^|status]
rem
rem Builds both binaries, bakes de_nuke out of your CS2 files if the scene is
rem missing or out of date, then opens the viewer.
rem
rem   (nothing)   build, bake if needed, open the viewer
rem   rebake      force the scene to be rebuilt, then open the viewer
rem   bake        force the scene to be rebuilt and stop - no window
rem   status      say whether the scene is current and stop - no build, no window
rem
rem Your CS2 install and the mapview project next door are read only. The only
rem things this writes are scenes\de_nuke.nkp and the build directory.
rem
rem Controls once the window is up:
rem   WASD          move           Q / E        down / up
rem   right mouse   hold to look   scroll       change speed
rem   Shift / Ctrl  sprint / creep F            frame the whole map
rem   Esc           quit
setlocal
cd /d "%~dp0"

set "SCENE=scenes\de_nuke.nkp"
set "NUKE_VPK=C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\csgo\maps\de_nuke.vpk"

if /i "%~1"=="status" goto :status

where cargo >nul 2>&1
if errorlevel 1 goto :nocargo

rem The first build compiles wgpu and its dependencies and takes about a minute.
rem Every build after that is seconds, so this is a one-time cost.
echo [nukeplant] building...
cargo build --release
if errorlevel 1 goto :fail

rem Every conditional below uses a parenthesised block. "if cond A & B" does not
rem bind B to the condition in cmd - B runs either way - which silently made the
rem launcher re-bake on every single click.
if /i "%~1"=="rebake" (
    echo [nukeplant] rebake requested
    goto :bake
)
if /i "%~1"=="bake" (
    echo [nukeplant] bake only, no window
    goto :bake
)

call :checkstale
if /i "%STALE%"=="missing" (
    echo [nukeplant] %SCENE% not found
    goto :bake
)
if /i "%STALE%"=="yes" (
    echo [nukeplant] the bake has changed since %SCENE% was written
    goto :bake
)
goto :launch

:bake
rem No parenthesised block here on purpose. The CS2 path contains "(x86)", and
rem cmd expands the variable before it parses the block, so the ")" in "(x86)"
rem closes the block early and the whole file fails to parse.
if not exist "%NUKE_VPK%" goto :novpk
echo [nukeplant] baking de_nuke from your CS2 files ^(read-only^)...
cargo run --release -p bake --bin nkp-bake
if errorlevel 1 goto :fail
if /i "%~1"=="bake" exit /b 0

:launch
echo.
echo [nukeplant] WASD to move, hold RIGHT MOUSE to look, Shift to sprint, F to frame, Esc to quit.
cargo run --release -p view --bin nukeplant -- "%SCENE%"
if errorlevel 1 goto :fail
exit /b 0

:status
call :checkstale
if /i "%STALE%"=="missing" (
    echo [nukeplant] %SCENE% has not been baked yet.
    exit /b 0
)
if /i "%STALE%"=="yes" (
    echo [nukeplant] %SCENE% is out of date - run nukeplant.bat rebake.
    exit /b 0
)
echo [nukeplant] %SCENE% is current.
exit /b 0

rem Decide whether the scene needs rebuilding: is anything the bake is made of
rem newer than the scene it wrote? Without this, editing the bake and clicking
rem the launcher would quietly open yesterday's geometry, which is the kind of
rem thing you only notice an hour in.
:checkstale
set "STALE=no"
if not exist "%SCENE%" (
    set "STALE=missing"
    goto :eof
)
rem One line, and no pipes. A "^|" inside for /f reaches PowerShell as a literal
rem "^|" rather than a pipe, so the newest-file scan is a plain foreach instead.
for /f %%s in ('powershell -NoProfile -ExecutionPolicy Bypass -Command "$s=(Get-Item '%SCENE%').LastWriteTime; $m=[datetime]::MinValue; foreach($f in @(Get-ChildItem crates\nkp-bake,crates\nkp-format -Recurse -File -ErrorAction SilentlyContinue)){ if($f.LastWriteTime -gt $m){ $m=$f.LastWriteTime } }; if($m -gt $s){'yes'}else{'no'}"') do set "STALE=%%s"
goto :eof

:nocargo
echo [nukeplant] cargo is not on PATH. Install Rust from https://rustup.rs
goto :fail

:novpk
echo.
echo [nukeplant] Could not find de_nuke.vpk at:
echo             %NUKE_VPK%
echo             CS2 has to be installed for the scene to be baked.
goto :fail

:fail
echo.
echo [nukeplant] Something failed - see the output above.
pause
exit /b 1
