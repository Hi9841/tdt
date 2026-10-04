# Download the Moondream Parakeet Redux model used by TDT.
[CmdletBinding()]
param(
    [ValidateSet("parakeet-redux", "all", "parakeet-unified-en-0.6b-int8", "parakeet-unified-en-0.6b-q8")]
    [string]$Model = "parakeet-redux",
    [string]$ModelsRoot = ""
)

$ErrorActionPreference = "Stop"
if (-not $ModelsRoot) {
    if ($PSScriptRoot) {
        $ModelsRoot = $PSScriptRoot
    } elseif ($MyInvocation.MyCommand.Path) {
        $ModelsRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
    } else {
        $ModelsRoot = Join-Path (Get-Location) "models"
    }
}
$Unified = @{
    Repo = "csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-1120ms"
    Dir = "parakeet-unified-en-0.6b-q8"
    Files = @(
        @{ Name = "encoder.int8.onnx"; Sha256 = "1C03F1192DE41771384AF22972CA10203613BA56197A024F275B86727CD35911" },
        @{ Name = "decoder.int8.onnx"; Sha256 = "34FEA72425D2506600772BA191A6D3F99C0710ABDB68D9A3DC89FA8CB2AA473A" },
        @{ Name = "joiner.int8.onnx"; Sha256 = "869F43F7D24595C55581AD3BF249A935FB8A71389FBDAA7504B9F46F93140F8A" },
        @{ Name = "tokens.txt"; Sha256 = "DC0B4584AB2E4DDBF888425C076C61B736E7356A015250DB7D307E6F1A8188FF" }
    )
}
$Redux = @{
    Repo = "moondream/parakeet-redux"
    Dir = "parakeet-redux"
    Files = @(
        @{ Name = "config.json"; Sha256 = "503C653B2E3BB788ADBCB04F5ABDEE532D958686564081BAEED133FF10143F6E" },
        @{ Name = "model.safetensors"; Sha256 = "78EC25733EE0D0C1586D1346FC86DB9D0C2E436E3A8AB1D32A82D1BB8F848D21" },
        @{ Name = "ternary.json"; Sha256 = "1221C6D3CE901FFE09C089DA758A8DB8B76189F80CFF41C5AFC244FC61E2051D" },
        @{ Name = "tokenizer.json"; Sha256 = "BD321B096832A3F270BD3B2A88823957920F1A5C5ADA71114A26EA729D0CBE91" }
    )
}
$Catalog = @{
    "parakeet-unified-en-0.6b-int8" = $Unified.Clone()
    "parakeet-unified-en-0.6b-q8" = $Unified.Clone()
    "parakeet-redux" = $Redux.Clone()
}
$Catalog["parakeet-unified-en-0.6b-int8"].Label = "Parakeet INT8"
$Catalog["parakeet-unified-en-0.6b-q8"].Label = "Parakeet Q8"
$Catalog["parakeet-redux"].Label = "Parakeet Redux"

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
    @("parakeet-unified-en-0.6b-int8")
} else {
    @($Model)
}

foreach ($id in $ids) {
    Install-TdtModel $id
}
