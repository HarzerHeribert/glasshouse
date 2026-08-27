@echo off
setlocal EnableExtensions
set "MODE=%~1"
if not defined MODE set "MODE=all"
set "OVERALL=0"

call "C:\BuildTools\VC\Auxiliary\Build\vcvarsarm64.bat" >nul
if errorlevel 1 exit /b %errorlevel%
set "PATH=C:\BuildTools\VC\Tools\Llvm\ARM64\bin;C:\Program Files\Git\bin;%PATH%"
set "CARGO_TARGET_DIR=C:\ci\target"
cd /d C:\ci\glasshouse

if /i "%MODE%"=="all" goto all
if /i "%MODE%"=="stable" goto stable
if /i "%MODE%"=="build" goto build_only
if /i "%MODE%"=="test" goto test_only
if /i "%MODE%"=="msrv" goto msrv_only
echo Usage: run-glasshouse-ci.cmd [all^|stable^|build^|test^|msrv]
exit /b 2

:all
call :build
call :test
call :msrv
goto done

:stable
call :build
call :test
goto done

:build_only
call :build
goto done

:test_only
call :test
goto done

:msrv_only
call :msrv
goto done

:build
echo.
echo === Windows ARM64 stable build ===
set "RUSTFLAGS=-D warnings"
cargo +stable build --locked --workspace --all-targets
if errorlevel 1 set "OVERALL=1"
exit /b 0

:test
echo.
echo === Windows ARM64 stable tests ===
set "RUSTFLAGS=-D warnings"
cargo +stable test --locked --workspace --no-fail-fast -- --nocapture
if errorlevel 1 set "OVERALL=1"
exit /b 0

:msrv
echo.
echo === Windows ARM64 MSRV 1.88 check ===
set "RUSTFLAGS="
cargo +1.88.0-aarch64-pc-windows-msvc check --locked --workspace --all-targets
if errorlevel 1 set "OVERALL=1"
exit /b 0

:done
echo.
if "%OVERALL%"=="0" echo Windows ARM64 CI passed.
if not "%OVERALL%"=="0" echo Windows ARM64 CI failed.
exit /b %OVERALL%
