@echo off
REM ============================================
REM  MiMo ASR Assistant Release Build
REM  Order matches E:\sevnX\release-build.bat:
REM  inject PATH -> frontend typecheck -> lint -> test
REM  -> tauri build -> verify artifacts
REM  PATH is set explicitly inside the script so it does
REM  not depend on stale terminal environment variables.
REM  ASCII-only on purpose: cmd.exe parses batch files using
REM  the system ANSI codepage, so non-ASCII text would be
REM  misread and break parsing.
REM ============================================
setlocal

set "NODE_DIR=C:\Program Files\nodejs"
set "CARGO_DIR=%USERPROFILE%\.cargo\bin"
set "PATH=%NODE_DIR%;%CARGO_DIR%;%PATH%"

set "ROOT=%~dp0"
set "NPMCMD=%NODE_DIR%\npm.cmd"
set "EXE=%~dp0src-tauri\target\release\mimo-asr-assistant.exe"
set "NSISDIR=%~dp0src-tauri\target\release\bundle\nsis"
set "CI=false"

echo ============================================
echo   MiMo ASR Assistant Release Build
echo ============================================
echo.

if not exist "%NODE_DIR%\node.exe" (
    echo [ERROR] Node.js not found: "%NODE_DIR%\node.exe"
    echo Install Node.js to the default directory or edit NODE_DIR in this script.
    goto :done
)
if not exist "%CARGO_DIR%\cargo.exe" (
    echo [ERROR] Cargo not found: "%CARGO_DIR%\cargo.exe"
    echo Install Rust via rustup or edit CARGO_DIR in this script.
    goto :done
)

echo [1/5] TypeScript check...
cd /d "%ROOT%"
call "%NPMCMD%" run typecheck
if %ERRORLEVEL% neq 0 (
    echo [ERROR] TypeScript check failed.
    goto :done
)

echo [2/5] ESLint check...
call "%NPMCMD%" run lint
if %ERRORLEVEL% neq 0 (
    echo [ERROR] ESLint check failed.
    goto :done
)

echo [3/5] Frontend unit tests...
call "%NPMCMD%" run test
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Frontend unit tests failed.
    goto :done
)

echo [4/5] Tauri release build (frontend + Rust + NSIS)...
REM Optionally kill a leftover process so the exe is not locked.
taskkill /IM mimo-asr-assistant.exe /F >nul 2>&1
call "%NPMCMD%" run tauri build
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Tauri release build failed.
    goto :done
)

echo [5/5] Verifying artifacts...
if not exist "%EXE%" (
    echo [ERROR] Release executable not found: "%EXE%"
    goto :done
)
if not exist "%NSISDIR%\*.exe" (
    echo [ERROR] NSIS installer not found in: "%NSISDIR%"
    goto :done
)

echo.
echo Release build completed: "%EXE%"
echo NSIS installer: "%NSISDIR%"

:done
echo.
pause