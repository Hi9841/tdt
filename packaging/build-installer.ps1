# Builds dist/TDT-Setup.exe, the one file a release ships.
#
# The setup stub (packaging/tdt-setup) carries TDT.exe compressed at its end:
# [stub][deflate(TDT.exe)][SHA-256 of TDT.exe: 32][version: 16][deflate length: 8][TDTSFX2\0]
[CmdletBinding()]
param(
    # The release tag. The build stops when it does not match desktop/Cargo.toml.
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Desktop = Join-Path $RepoRoot "desktop"
$SetupCrate = Join-Path $PSScriptRoot "tdt-setup"
$Dist = Join-Path $RepoRoot "dist"

$CargoVersion = (Select-String -Path (Join-Path $Desktop "Cargo.toml") -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if ($Version -and $Version.TrimStart("vV") -ne $CargoVersion) {
    throw "Tag $Version does not match desktop/Cargo.toml version $CargoVersion"
}
if ([Text.Encoding]::ASCII.GetByteCount($CargoVersion) -gt 16) {
    throw "Version $CargoVersion is longer than 16 bytes"
}

Write-Host "Building TDT $CargoVersion..."
& cargo build --release --locked --manifest-path (Join-Path $Desktop "Cargo.toml") --bin TDT
if ($LASTEXITCODE -ne 0) { throw "TDT build failed" }
& cargo build --release --locked --manifest-path (Join-Path $SetupCrate "Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "Setup build failed" }

$app = [IO.File]::ReadAllBytes((Join-Path $Desktop "target\release\TDT.exe"))
$stub = [IO.File]::ReadAllBytes((Join-Path $SetupCrate "target\release\tdt-setup.exe"))

$packed = New-Object IO.MemoryStream
$deflate = New-Object IO.Compression.DeflateStream($packed, [IO.Compression.CompressionLevel]::Optimal, $true)
$deflate.Write($app, 0, $app.Length)
$deflate.Dispose()
$packedBytes = $packed.ToArray()

$sha = [Security.Cryptography.SHA256]::Create()
$hash = $sha.ComputeHash($app)
$versionBytes = New-Object byte[] 16
[Text.Encoding]::ASCII.GetBytes($CargoVersion).CopyTo($versionBytes, 0)

if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory -Path $Dist | Out-Null
$setup = Join-Path $Dist "TDT-Setup.exe"
$out = [IO.File]::Open($setup, [IO.FileMode]::Create)
try {
    foreach ($part in @(
            $stub,
            $packedBytes,
            $hash,
            $versionBytes,
            [BitConverter]::GetBytes([int64]$packedBytes.Length),
            [Text.Encoding]::ASCII.GetBytes("TDTSFX2`0")
        )) {
        $out.Write($part, 0, $part.Length)
    }
} finally {
    $out.Dispose()
}
$size = [math]::Round((Get-Item $setup).Length / 1MB, 1)
Write-Host "Wrote $setup ($size MB)"

# TDT 0.2.32 and older only offer an update that SHA256SUMS.txt lists.
# Keep publishing it until those versions are gone.
$setupHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $setup).Hash.ToLowerInvariant()
Set-Content -LiteralPath (Join-Path $Dist "SHA256SUMS.txt") -Value "$setupHash  TDT-Setup.exe"
