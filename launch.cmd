@echo off
rem ---------------------------------------------------------------------------
rem Launch MetalVJ on Windows.
rem
rem Builds `metal-dash` (incremental, so a repeat run is quick) and then opens
rem the native window. Any arguments are passed straight through, so
rem `launch.cmd --port 7911` works the same as running the binary directly.
rem
rem CMake is needed by a native dependency; cargo finds it on PATH. If the
rem build fails with "CMAKE not found", install CMake and reopen the terminal.
rem ---------------------------------------------------------------------------
setlocal

set "ROOT=%~dp0"
pushd "%ROOT%" || exit /b 1

if not defined CARGO_HOME set "CARGO_HOME=%USERPROFILE%\.cargo"
set "PATH=%CARGO_HOME%\bin;%PATH%"

where cargo >nul 2>nul
if errorlevel 1 (
  echo.
  echo   cargo was not found on PATH.
  echo   Install the Rust toolchain from https://rustup.rs then reopen this window.
  echo.
  popd
  pause
  exit /b 1
)

echo Building metal-dash...
cargo build -p metal-dash
if errorlevel 1 (
  echo.
  echo   Build failed. The compiler output above says why.
  echo.
  popd
  pause
  exit /b 1
)

set "EXE=%ROOT%target\debug\metal-dash.exe"
if not exist "%EXE%" (
  echo.
  echo   The build reported success but %EXE% is missing.
  echo.
  popd
  pause
  exit /b 1
)

echo Starting MetalVJ...
"%EXE%" %*

popd
endlocal
