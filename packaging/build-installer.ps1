# Build a portable zip and a self-contained TDT-Setup.exe (no Inno Setup required).
[CmdletBinding()]
param(
    [string]$RepoRoot = "",
    [string]$Version = "0.1.7"
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
if (-not $ScriptDir -and $MyInvocation.MyCommand.Path) {
    $ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
}
if (-not $ScriptDir) {
    $ScriptDir = Join-Path (Get-Location) "packaging"
}
if (-not $RepoRoot) {
    $RepoRoot = (Resolve-Path (Join-Path $ScriptDir "..")).Path
}
$Version = $Version.TrimStart("vV")
$Desktop = Join-Path $RepoRoot "desktop"
$SetupCrate = Join-Path $ScriptDir "tdt-setup"
$Stage = Join-Path $ScriptDir "stage"
$Dist = Join-Path $RepoRoot "dist"
$ReleaseExe = Join-Path $Desktop "target\release\TDT.exe"
$StubExe = Join-Path $SetupCrate "target\release\tdt-setup.exe"

Write-Host "Building TDT $Version..."
$appBuild = Start-Process cargo -ArgumentList @('build', '--release', '--locked', '--manifest-path', ('"{0}"' -f (Join-Path $Desktop 'Cargo.toml'))) -NoNewWindow -Wait -PassThru
if ($appBuild.ExitCode -ne 0) { throw "cargo build --release failed" }

$setupBuild = Start-Process cargo -ArgumentList @('build', '--release', '--locked', '--manifest-path', ('"{0}"' -f (Join-Path $SetupCrate 'Cargo.toml'))) -NoNewWindow -Wait -PassThru
if ($setupBuild.ExitCode -ne 0) { throw "tdt-setup build failed" }

if (-not (Test-Path $ReleaseExe)) {
    throw "Missing $ReleaseExe"
}
if (-not (Test-Path $StubExe)) {
    throw "Missing $StubExe"
}

if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
New-Item -ItemType Directory -Path $Stage -Force | Out-Null
New-Item -ItemType Directory -Path $Dist -Force | Out-Null

Copy-Item $ReleaseExe (Join-Path $Stage "TDT.exe")
Copy-Item (Join-Path $RepoRoot "LICENSE") $Stage
Copy-Item (Join-Path $RepoRoot "NOTICE") $Stage
Copy-Item (Join-Path $RepoRoot "THIRD_PARTY_NOTICES.md") $Stage
Copy-Item (Join-Path $RepoRoot "README.md") $Stage
Copy-Item (Join-Path $ScriptDir "tdt-setup.ps1") (Join-Path $Stage "Install-TDT.ps1")
Set-Content -LiteralPath (Join-Path $Stage "VERSION") -Value $Version -NoNewline

# Never bundle SenseVoice. First run downloads it from Settings. Updates
# must not reinstall a 200 MB model the user already has.
$portable = Join-Path $Dist "TDT-$Version-windows-x64.zip"
if (Test-Path $portable) { Remove-Item $portable -Force }
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $portable -Force
Write-Host "Wrote $portable"

Copy-Item $ReleaseExe (Join-Path $Dist "TDT.exe") -Force

$payload = Join-Path $ScriptDir "payload.zip"
if (Test-Path $payload) { Remove-Item $payload -Force }
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $payload -Force

$setup = Join-Path $Dist "TDT-Setup.exe"
if (Test-Path $setup) { Remove-Item $setup -Force }
$setupStream = [System.IO.File]::Open($setup, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
try {
    $stubStream = [System.IO.File]::OpenRead($StubExe)
    try { $stubStream.CopyTo($setupStream) } finally { $stubStream.Dispose() }
    $zipStream = [System.IO.File]::OpenRead($payload)
    try {
        $zipLen = $zipStream.Length
        $zipStream.CopyTo($setupStream)
    } finally { $zipStream.Dispose() }
    $sizeBytes = [BitConverter]::GetBytes([int64]$zipLen)
    $setupStream.Write($sizeBytes, 0, 8)
    $magic = [byte[]](0x54, 0x44, 0x54, 0x5A, 0x49, 0x50, 0x31, 0x00)
    $setupStream.Write($magic, 0, 8)
} finally {
    $setupStream.Dispose()
}
Remove-Item $payload -Force
Write-Host "Wrote $setup"

Copy-Item (Join-Path $Stage "Install-TDT.ps1") (Join-Path $Dist "Install-TDT.ps1") -Force

$sums = Join-Path $Dist "SHA256SUMS.txt"
$lines = @()
Get-ChildItem $Dist -File | Where-Object { $_.Name -ne "SHA256SUMS.txt" } | ForEach-Object {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
    $lines += "$hash  $($_.Name)"
}
Set-Content -LiteralPath $sums -Value $lines
Write-Host "Wrote $sums"
