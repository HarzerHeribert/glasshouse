@echo off
setlocal EnableExtensions
set "MODE=%~1"
if not defined MODE set "MODE=all"
set "OVERALL=0"

call "C:\BuildTools\VC\Auxiliary\Build\vcvarsarm64.bat" >nul
if errorlevel 1 exit /b %errorlevel%
set "PATH=C:\BuildTools\VC\Tools\Llvm\ARM64\bin;C:\Program Files\Git\bin;%PATH%"
set "CARGO_TARGET_DIR=C:\ci\target"
rem The VM disk is 85 GB. A workspace build with full debug info wrote 27.6 GB
rem of PDBs for 4.7 GB of test executables and 7 GB of incremental caches, and
rem filled it twice on 2026-09-17. Line tables keep file:line in every panic
rem and backtrace, which is what a red is read from; a fresh copy of the tree
rem gains little from incremental caches. The GitHub cells keep the defaults.
set "CARGO_PROFILE_DEV_DEBUG=line-tables-only"
set "CARGO_INCREMENTAL=0"
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
cargo +stable build --locked --workspace --all-targets
if errorlevel 1 set "OVERALL=1"
exit /b 0

:test
echo.
echo === Windows ARM64 stable tests ===
cargo +stable test --locked --workspace --no-fail-fast -- --nocapture
if errorlevel 1 set "OVERALL=1"
exit /b 0

:msrv
echo.
echo === Windows ARM64 MSRV 1.88 check ===
cargo +1.88.0-aarch64-pc-windows-msvc check --locked --workspace --all-targets
if errorlevel 1 set "OVERALL=1"
exit /b 0

:done
echo.
if "%OVERALL%"=="0" echo Windows ARM64 CI passed.
if not "%OVERALL%"=="0" echo Windows ARM64 CI failed.
exit /b %OVERALL%
