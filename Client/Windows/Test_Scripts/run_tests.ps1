# Alle Client-Tests auf Windows ausfuehren (WDAC-sicher via cargo.ps1).
# Aufruf: powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\run_tests.ps1
$ErrorActionPreference = "Continue"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "=== Unit-Tests (Client) ==="
& "$ScriptDir\cargo.ps1" -Retries 10 test -p rusttalk-client

Write-Host ""
Write-Host "=== Release-Build (Client) ==="
& "$ScriptDir\cargo.ps1" -Retries 10 build --release -p rusttalk-client

Write-Host ""
Write-Host "FERTIG. GUI-Client: $env:LOCALAPPDATA\Programs\RustTalk-src\target\release\rusttalk.exe"
