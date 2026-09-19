# Per-user TDT installer. Does not need admin.
[CmdletBinding()]
param(
    [string]$StageDir = "",
    [string]$Version = "",
    [switch]$Uninstall,
    [switch]$CloseApplications,
    [switch]$SkipLaunch
)

$ErrorActionPreference = "Stop"
$AppName = "TDT"
$InstallDir = Join-Path $env:LOCALAPPDATA $AppName
$UninstallReg = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\TDT"
$LogPath = Join-Path $env:TEMP "TDT-setup.log"

function Write-SetupLog([string]$Message) {
    $line = "$(Get-Date -Format o) $Message"
    Add-Content -LiteralPath $LogPath -Value $line -ErrorAction SilentlyContinue
    Write-Host $Message
}

function Stop-TdtProcesses {
    Get-Process -Name TDT, voice-stt-desktop -ErrorAction SilentlyContinue | ForEach-Object {
        Write-SetupLog "Stopping $($_.ProcessName) pid=$($_.Id)"
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 400
}

if (-not $Version) {
    $versionFile = Join-Path $PSScriptRoot "VERSION"
    if ($StageDir -and (Test-Path (Join-Path $StageDir "VERSION"))) {
        $versionFile = Join-Path $StageDir "VERSION"
    }
    if (Test-Path $versionFile) {
        $Version = (Get-Content -LiteralPath $versionFile -Raw).Trim()
    } else {
        $Version = "0.1.0"
    }
}
$Version = $Version.TrimStart("vV")

if ($Uninstall) {
    Stop-TdtProcesses
    Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\TDT" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup\TDT.lnk") -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $env:USERPROFILE "Desktop\TDT.lnk") -Force -ErrorAction SilentlyContinue
    Remove-Item "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run\TDT" -Force -ErrorAction SilentlyContinue
    Remove-Item "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run\TDT" -Force -ErrorAction SilentlyContinue
    Remove-Item "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder\TDT.lnk" -Force -ErrorAction SilentlyContinue
    Remove-Item $UninstallReg -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-SetupLog "TDT removed."
    exit 0
}

if (-not $StageDir) {
    if ($PSScriptRoot) {
        $StageDir = $PSScriptRoot
    } else {
        $StageDir = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) "stage"
    }
}
if (-not (Test-Path (Join-Path $StageDir "TDT.exe"))) {
    throw "Missing staged TDT.exe. Run packaging/build-installer.ps1 first."
}

Stop-TdtProcesses

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
# Never copy models/. Updates and reinstalls must leave a downloaded
# SenseVoice tree on disk.
Get-ChildItem -LiteralPath $StageDir -Force | Where-Object { $_.Name -ne "models" } | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $InstallDir $_.Name) -Recurse -Force
}

$uninstaller = Join-Path $InstallDir "uninstall-tdt.ps1"
Copy-Item $PSCommandPath $uninstaller -Force
Set-Content -LiteralPath (Join-Path $InstallDir "VERSION") -Value $Version -NoNewline

$programs = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\TDT"
New-Item -ItemType Directory -Path $programs -Force | Out-Null
$ws = New-Object -ComObject WScript.Shell
$shortcut = $ws.CreateShortcut((Join-Path $programs "TDT.lnk"))
$shortcut.TargetPath = Join-Path $InstallDir "TDT.exe"
$shortcut.WorkingDirectory = $InstallDir
$shortcut.Description = "TDT - Talk Don't Type"
$shortcut.Save()

New-Item -Path $UninstallReg -Force | Out-Null
Set-ItemProperty $UninstallReg DisplayName "TDT - Talk Don't Type"
Set-ItemProperty $UninstallReg DisplayVersion $Version
Set-ItemProperty $UninstallReg Publisher "Hi9841"
Set-ItemProperty $UninstallReg InstallLocation $InstallDir
Set-ItemProperty $UninstallReg UninstallString "powershell.exe -ExecutionPolicy Bypass -File `"$uninstaller`" -Uninstall"
Set-ItemProperty $UninstallReg DisplayIcon (Join-Path $InstallDir "TDT.exe")
Set-ItemProperty $UninstallReg URLInfoAbout "https://github.com/Hi9841/tdt"
Set-ItemProperty $UninstallReg NoModify 1
Set-ItemProperty $UninstallReg NoRepair 1
$exe = Get-Item (Join-Path $InstallDir "TDT.exe")
Set-ItemProperty $UninstallReg EstimatedSize ([int]($exe.Length / 1KB))

Write-SetupLog "Installed TDT $Version to $InstallDir"
if (-not $SkipLaunch) {
    Start-Process -FilePath (Join-Path $InstallDir "TDT.exe") -WorkingDirectory $InstallDir
}
