# Download the SenseVoice model used by both desktop and Android builds.
[CmdletBinding()]
param(
    [string]$TargetDir = (Join-Path $PSScriptRoot "sensevoice")
)

$ErrorActionPreference = "Stop"
$BaseUrl = "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main"
$Files = @(
    @{ Name = "tokens.txt"; Sha256 = "F449EB28DC567533D7FA59BE34E2ABCA8784F771850C78A47FB731A31429A1DC" },
    @{ Name = "model.int8.onnx"; Sha256 = "C71F0CE00BEC95B07744E116345E33D8CBBE08CEF896382CF907BF4B51A2CD51" },
    @{ Name = "LICENSE"; Sha256 = $null },
    @{ Name = "README.md"; Sha256 = $null }
)

New-Item -ItemType Directory -Path $TargetDir -Force | Out-Null

foreach ($file in $Files) {
    $destination = Join-Path $TargetDir $file.Name
    if (-not (Test-Path -LiteralPath $destination)) {
        $temporary = "$destination.download"
        Write-Host "Downloading $($file.Name)..." -ForegroundColor Yellow
        Invoke-WebRequest -Uri "$BaseUrl/$($file.Name)?download=true" -OutFile $temporary
        Move-Item -LiteralPath $temporary -Destination $destination -Force
    }

    if ($file.Sha256) {
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash.ToUpperInvariant()
        if ($actual -ne $file.Sha256) {
            throw "SHA256 mismatch for $($file.Name): expected $($file.Sha256), got $actual"
        }
    }
}

Write-Host "SenseVoice model is ready in $TargetDir" -ForegroundColor Green
