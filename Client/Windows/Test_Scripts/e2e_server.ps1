# E2E-Test von diesem Windows-Rechner gegen den Debian-Server.
# Startet zwei test_client-Instanzen, die sich ueber den Server Audio schicken.
#
# Aufruf: powershell -ExecutionPolicy Bypass -File Client\Windows\Test_Scripts\e2e_server.ps1 [-Server "10.69.40.101:7331"]
param(
    [string]$Server = "10.69.40.101:7331"
)
$ErrorActionPreference = "Continue"

$TestClient = "$env:LOCALAPPDATA\Programs\RustTalk-src\target\release\test_client.exe"
if (-not (Test-Path $TestClient)) {
    Write-Host "test_client fehlt -> Release-Build des Servers wird erstellt..."
    $ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    & "$ScriptDir\cargo.ps1" -Retries 10 build --release -p rusttalk-server
}

$o1 = "$env:TEMP\rusttalk-e2e-a.txt"
$o2 = "$env:TEMP\rusttalk-e2e-b.txt"
Remove-Item $o1, $o2 -Force -ErrorAction SilentlyContinue

Write-Host "=== Zwei Testclients gegen $Server (joinen ersten Voice-Channel) ==="
$p1 = Start-Process -FilePath $TestClient -ArgumentList "--addr",$Server,"--name","WinA","--join-kind","voice","--audio-secs","3","--duration-secs","6" -PassThru -NoNewWindow -RedirectStandardOutput $o1
Start-Sleep -Milliseconds 400
$p2 = Start-Process -FilePath $TestClient -ArgumentList "--addr",$Server,"--name","WinB","--join-kind","voice","--audio-secs","3","--duration-secs","6" -PassThru -NoNewWindow -RedirectStandardOutput $o2
$p1.WaitForExit()
$p2.WaitForExit()

Write-Host "--- Client A ---"; Get-Content $o1
Write-Host "--- Client B ---"; Get-Content $o2

$rxA = 0; $rxB = 0
if (Select-String -Path $o1 -Pattern 'AUDIO_RX=(\d+)' | ForEach-Object { $_.Matches[0].Groups[1].Value } | Where-Object { $_ }) { $rxA = [int]((Select-String -Path $o1 -Pattern 'AUDIO_RX=(\d+)').Matches[0].Groups[1].Value) }
if (Select-String -Path $o2 -Pattern 'AUDIO_RX=(\d+)' | ForEach-Object { $_.Matches[0].Groups[1].Value } | Where-Object { $_ }) { $rxB = [int]((Select-String -Path $o2 -Pattern 'AUDIO_RX=(\d+)').Matches[0].Groups[1].Value) }

if ($rxA -ge 10 -and $rxB -ge 10) {
    Write-Host "E2E OK: A hoerte $rxA Pakete von B, B hoerte $rxB Pakete von A"
    exit 0
} else {
    Write-Host "E2E FEHLGESCHLAGEN (RX_A=$rxA, RX_B=$rxB)"
    exit 1
}
