# Download Sherpa-ONNX speech models used by TDT.
# Default is the recommended Parakeet Q8 layout at models/parakeet-unified-en-0.6b-q8.
[CmdletBinding()]
param(
    [ValidateSet("moonshine-medium-streaming", "sensevoice-full", "parakeet-unified-en-0.6b-q8", "whisper-medium", "all")]
    [string]$Model = "parakeet-unified-en-0.6b-q8",
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
$Catalog = @{
    "moonshine-medium-streaming" = @{
        Label = "Moonshine Medium"
        Repo = "csukuangfj/sherpa-onnx-moonshine-base-en-int8"
        Dir = "moonshine-medium-streaming"
        Files = @(
            @{ Name = "preprocess.onnx"; Sha256 = "FFA630D395C5CCF76F5D4954BE5B882DF76AAF6491519EC01FD82EA7A3819FB2" },
            @{ Name = "encode.int8.onnx"; Sha256 = "7E38770F776F2E5583A53B052936005DF2BA5C833D7E09C2A5FD796B94BF73E2" },
            @{ Name = "uncached_decode.int8.onnx"; Sha256 = "C01F4B35093BCAC20D352D23A75A539E772964579F9D024A90E5E6F09CAE9987" },
            @{ Name = "cached_decode.int8.onnx"; Sha256 = "2DB74E51CEDF64A8B1BE3C8192E0BB5E4923AF0E90BD9E87F8E8771873F8EA03" },
            @{ Name = "tokens.txt"; Sha256 = "1165C2AEB9F72F457A83BE2D459A09054F27490ACD9B41BD43794DFD25E296EA" }
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
    "parakeet-unified-en-0.6b-q8" = @{
        Label = "Parakeet Q8"
        Repo = "csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-1120ms"
        Dir = "parakeet-unified-en-0.6b-q8"
        Files = @(
            @{ Name = "encoder.int8.onnx"; Sha256 = "1C03F1192DE41771384AF22972CA10203613BA56197A024F275B86727CD35911" },
            @{ Name = "decoder.int8.onnx"; Sha256 = "34FEA72425D2506600772BA191A6D3F99C0710ABDB68D9A3DC89FA8CB2AA473A" },
            @{ Name = "joiner.int8.onnx"; Sha256 = "869F43F7D24595C55581AD3BF249A935FB8A71389FBDAA7504B9F46F93140F8A" },
            @{ Name = "tokens.txt"; Sha256 = "DC0B4584AB2E4DDBF888425C076C61B736E7356A015250DB7D307E6F1A8188FF" }
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
    @("moonshine-medium-streaming", "sensevoice-full", "parakeet-unified-en-0.6b-q8", "whisper-medium")
} else {
    @($Model)
}

foreach ($id in $ids) {
    Install-TdtModel $id
}
