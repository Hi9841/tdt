# Download Sherpa-ONNX speech models used by TDT.
# Default keeps the bundled SenseVoice Small layout at models/sensevoice.
[CmdletBinding()]
param(
    [ValidateSet("sensevoice-small", "sensevoice-full", "whisper-small", "whisper-medium", "all")]
    [string]$Model = "sensevoice-small",
    [string]$ModelsRoot = $PSScriptRoot
)

$ErrorActionPreference = "Stop"
$Catalog = @{
    "sensevoice-small" = @{
        Label = "SenseVoice Small"
        Repo = "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17"
        Dir = "sensevoice"
        Files = @(
            @{ Name = "tokens.txt"; Sha256 = "F449EB28DC567533D7FA59BE34E2ABCA8784F771850C78A47FB731A31429A1DC" },
            @{ Name = "model.int8.onnx"; Sha256 = "C71F0CE00BEC95B07744E116345E33D8CBBE08CEF896382CF907BF4B51A2CD51" },
            @{ Name = "LICENSE"; Sha256 = $null },
            @{ Name = "README.md"; Sha256 = $null }
        )
    }
    "sensevoice-full" = @{
        Label = "SenseVoice Full"
        Repo = "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17"
        Dir = "sensevoice"
        Files = @(
            @{ Name = "tokens.txt"; Sha256 = "F449EB28DC567533D7FA59BE34E2ABCA8784F771850C78A47FB731A31429A1DC" },
            @{ Name = "model.onnx"; Sha256 = "977016BD9C79F9EB343430B5CC305E07AB64D5212DFF41B0DCFA1694BEE9A8CB" },
            @{ Name = "LICENSE"; Sha256 = $null },
            @{ Name = "README.md"; Sha256 = $null }
        )
    }
    "whisper-small" = @{
        Label = "Whisper Small"
        Repo = "csukuangfj/sherpa-onnx-whisper-small"
        Dir = "whisper-small"
        Files = @(
            @{ Name = "small-encoder.int8.onnx"; Sha256 = "4CBE7B22FA9026B843B60A68640C747DE05BAFB1A11B57EDC0E66C232D9F33A9" },
            @{ Name = "small-decoder.int8.onnx"; Sha256 = "ACAD50B5C782696E91B55914CC5AB4F756F1532F76E22AA6FC615F39FB69A8EE" },
            @{ Name = "small-tokens.txt"; Sha256 = "B34B360DBB493E781E479794586D661700670D65564001F23024971D1F2FA126" }
        )
    }
    "whisper-medium" = @{
        Label = "Whisper Medium"
        Repo = "csukuangfj/sherpa-onnx-whisper-medium"
        Dir = "whisper-medium"
        Files = @(
            @{ Name = "medium-encoder.int8.onnx"; Sha256 = "1C54582B4D829DE0089F6CB63BBBDB3BF7555398BACAF855FBECF1A84DFD193E" },
            @{ Name = "medium-decoder.int8.onnx"; Sha256 = "595D00A338A365A7BFA0CA7F296CABC639583BEF770AB6130DF90F49A6412747" },
            @{ Name = "medium-tokens.txt"; Sha256 = "B34B360DBB493E781E479794586D661700670D65564001F23024971D1F2FA126" }
        )
    }
}

function Install-TdtModel([string]$Id) {
    $spec = $Catalog[$Id]
    if (-not $spec) {
        throw "Unknown model '$Id'."
    }
    $targetDir = Join-Path $ModelsRoot $spec.Dir
    $baseUrl = "https://huggingface.co/$($spec.Repo)/resolve/main"
    New-Item -ItemType Directory -Path $targetDir -Force | Out-Null

    foreach ($file in $spec.Files) {
        $destination = Join-Path $targetDir $file.Name
        if (-not (Test-Path -LiteralPath $destination)) {
            $temporary = "$destination.download"
            Write-Host "Downloading $($file.Name)..." -ForegroundColor Yellow
            Invoke-WebRequest -Uri "$baseUrl/$($file.Name)?download=true" -OutFile $temporary
            Move-Item -LiteralPath $temporary -Destination $destination -Force
        }

        if ($file.Sha256) {
            $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash.ToUpperInvariant()
            if ($actual -ne $file.Sha256) {
                throw "SHA256 mismatch for $($file.Name): expected $($file.Sha256), got $actual"
            }
        }
    }

    Write-Host "$($spec.Label) is ready in $targetDir" -ForegroundColor Green
}

$ids = if ($Model -eq "all") {
    @("sensevoice-small", "sensevoice-full", "whisper-small", "whisper-medium")
} else {
    @($Model)
}

foreach ($id in $ids) {
    Install-TdtModel $id
}
