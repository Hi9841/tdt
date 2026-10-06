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
$Redux = @{
    Repo = "moondream/parakeet-redux"
    Dir = "parakeet-redux"
    Label = "Parakeet Redux"
    Files = @(
        @{ Name = "config.json"; Sha256 = "503C653B2E3BB788ADBCB04F5ABDEE532D958686564081BAEED133FF10143F6E" },
        @{ Name = "model.safetensors"; Sha256 = "78EC25733EE0D0C1586D1346FC86DB9D0C2E436E3A8AB1D32A82D1BB8F848D21" },
        @{ Name = "ternary.json"; Sha256 = "1221C6D3CE901FFE09C089DA758A8DB8B76189F80CFF41C5AFC244FC61E2051D" },
        @{ Name = "tokenizer.json"; Sha256 = "BD321B096832A3F270BD3B2A88823957920F1A5C5ADA71114A26EA729D0CBE91" }
    )
}
$Catalog = @{
    "parakeet-redux" = $Redux
    "parakeet-unified-en-0.6b-int8" = $Redux
    "parakeet-unified-en-0.6b-q8" = $Redux
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
            $ok = $false
            for ($attempt = 1; $attempt -le 3 -and -not $ok; $attempt++) {
                try {
                    Invoke-WebRequest -UseBasicParsing -Uri "$baseUrl/$($file.Name)?download=true" -OutFile $temporary
                    $ok = $true
                } catch {
                    Write-Host "Attempt $attempt failed: $_" -ForegroundColor DarkYellow
                    Start-Sleep -Seconds $attempt
                }
            }
            if (-not $ok) { throw "Could not download $($file.Name)." }

            if ($file.Sha256) {
                $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $temporary).Hash.ToUpperInvariant()
                if ($actual -ne $file.Sha256) {
                    Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
                    throw "SHA256 mismatch for $($file.Name): expected $($file.Sha256), got $actual"
                }
            }
            Move-Item -LiteralPath $temporary -Destination $destination -Force
        }

        if ($file.Sha256) {
            $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash.ToUpperInvariant()
            if ($actual -ne $file.Sha256) {
                # Delete the bad file so the next run re-downloads it.
                Remove-Item -LiteralPath $destination -Force -ErrorAction SilentlyContinue
                throw "SHA256 mismatch for $($file.Name): expected $($file.Sha256), got $actual"
            }
        }
    }

    Write-Host "$($spec.Label) is ready in $targetDir" -ForegroundColor Green
}

$ids = if ($Model -eq "all") {
    # Redux is the only published model; the legacy aliases point at it.
    @("parakeet-redux")
} else {
    @($Model)
}

foreach ($id in $ids) {
    Install-TdtModel $id
}
