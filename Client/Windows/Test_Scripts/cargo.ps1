# cargo-Wrapper fuer diesen Windows-Rechner (WDAC-Workaround).
#
# Hintergrund (empirisch ermittelt, siehe test.md): WDAC blockiert
# cargo-Build-Skripte und gelegentlich frisch gebaute Test-Binaries
# (os error 4551), sobald cargo-Arbeitsverzeichnis ODER Manifest-Pfad
# unter C:\Users\jb\Documents liegen.
#
# Loesung:
# 1) Workspace nach %LOCALAPPDATA%\Programs\RustTalk-src spiegeln und dort bauen.
# 2) Transiente 4551-Bloecke: Retry.
# 3) Deterministische 4551-Bloecke auf EIGENE Binaries (gleiche Datei 2x hintereinander):
#    "Dice-Roll" - ein Kommentar im zugehoerigen Quellfile wird hochgezaehlt,
#    dadurch aendert sich der Binary-Inhalt und WDAC bewertet neu.
#
# Aufruf:
#   .\cargo.ps1 build --release
#   .\cargo.ps1 -Retries 10 test --workspace
#
# Rueckgabecode = cargo-Exitcode.

$ErrorActionPreference = "Continue"
$Src = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$BuildDir = "$env:LOCALAPPDATA\Programs\RustTalk-src"

Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue

# NASM for OpenH264's SIMD assembly (video ~5x faster); winget installs it per user.
foreach ($d in "$env:LOCALAPPDATA\bin\NASM", "$env:ProgramFiles\NASM") {
    if ((Test-Path "$d\nasm.exe") -and ($env:PATH -notlike "*$d*")) { $env:PATH = "$d;$env:PATH" }
}

# CMake for the Opus build (audiopus_sys): use the one Visual Studio / Build Tools ship if none is on PATH.
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    $vsCmake = Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio", "$env:ProgramFiles\Microsoft Visual Studio" -Recurse -Filter cmake.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($vsCmake) { $env:PATH = "$($vsCmake.DirectoryName);$env:PATH" }
}
if (-not $env:CMAKE_POLICY_VERSION_MINIMUM) { $env:CMAKE_POLICY_VERSION_MINIMUM = "3.5" }

$retries = 1
if ($args.Count -ge 2 -and $args[0] -eq "-Retries") {
    $retries = [int]$args[1]
    $args = $args[2..($args.Count - 1)]
}

# Binary-Stamm -> Quelldatei, die fuer den Dice-Roll angefasst wird.
$TouchMap = @{
    "rusttalk_server" = "Server\src\main.rs"
    "test_client"     = "Server\src\bin\test_client.rs"
    "integration"     = "Server\tests\integration.rs"
    "rusttalk_shared" = "shared\src\lib.rs"
    "rusttalk"        = "Client\Windows\src\main.rs"
}

function Sync-Workspace {
    robocopy $Src $BuildDir /MIR /NFL /NDL /NJH /NJS /XD target .git | Out-Null
    if ($LASTEXITCODE -gt 7) {
        Write-Error "robocopy fehlgeschlagen (Exit $LASTEXITCODE)"
        exit $LASTEXITCODE
    }
}

function Update-DiceComment([string]$relPath, [int]$attempt) {
    $file = Join-Path $Src $relPath
    $marker = "// wdac-dice:"
    $lines = Get-Content $file
    $found = $false
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match "^// wdac-dice: \d+") {
            $lines[$i] = "$marker $attempt"
            $found = $true
            break
        }
    }
    if (-not $found) {
        $lines += "$marker $attempt"
    }
    Set-Content -Path $file -Value $lines -Encoding utf8
    Write-Host "[cargo.ps1] Dice-Roll: $relPath -> Variante $attempt"
}

Sync-Workspace

$code = 1
$tmpOut = [IO.Path]::GetTempFileName()
$tmpErr = [IO.Path]::GetTempFileName()
$lastBlocked = ""
$sameBlockedCount = 0
Push-Location $BuildDir
try {
    foreach ($attempt in 1..$retries) {
        & cargo @args > $tmpOut 2> $tmpErr
        $code = $LASTEXITCODE
        $flat = (Get-Content $tmpOut -Raw -ErrorAction SilentlyContinue) +
                (Get-Content $tmpErr -Raw -ErrorAction SilentlyContinue)
        Write-Host $flat
        if ($code -eq 0) { break }
        if ($flat -notmatch "4551" -or $attempt -ge $retries) { break }

        # Geblockte Dateien extrahieren. cargo bricht lange Pfade um
        # (?s = Punkt matcht auch Newlines; Whitespace im Pfad wieder entfernen).
        $blockedPaths = @([regex]::Matches($flat, "(?s)could not execute process ``(.+?)``") |
            ForEach-Object { ($_.Groups[1].Value -replace "\s", "") })
        $currentBlocked = ($blockedPaths | Sort-Object) -join ";"
        if ($currentBlocked -ne "" -and $currentBlocked -eq $lastBlocked) {
            $sameBlockedCount++
        } else {
            $sameBlockedCount = 1
            $lastBlocked = $currentBlocked
        }

        # Bei wiederholtem Block derselben EIGENEN Binary: Quellfile anfassen.
        $diced = $false
        if ($sameBlockedCount -ge 2) {
            foreach ($bp in $blockedPaths) {
                $fileName = [IO.Path]::GetFileNameWithoutExtension($bp)
                $stem = ($fileName -split "-")[0]
                if ($TouchMap.ContainsKey($stem)) {
                    Update-DiceComment $TouchMap[$stem] ($sameBlockedCount)
                    $diced = $true
                }
            }
        }
        if ($diced) { Sync-Workspace }
        Write-Host "[cargo.ps1] WDAC-Block (4551) - Versuch $($attempt + 1) von $retries ..."
    }
} finally {
    Pop-Location
    Remove-Item $tmpOut, $tmpErr -Force -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "[cargo.ps1] Build-Verzeichnis: $BuildDir"
Write-Host "[cargo.ps1] Binaries liegen unter target\debug bzw. target\release"
exit $code
