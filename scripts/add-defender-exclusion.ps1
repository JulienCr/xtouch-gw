#Requires -Version 5.1
# Adds this repository's root to Microsoft Defender's exclusion list so pnpm
# junctions (and other dev tooling) aren't blocked by reparse-point protection.
# Self-elevates if not already running as Administrator.
#
# Defaults to the repo root resolved from $PSScriptRoot. Pass -ExclusionPath
# to override (e.g. to exclude a broader dev parent like 'D:\dev').

[CmdletBinding()]
param(
    [string]$ExclusionPath = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)

$currentUser = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
$isAdmin = $currentUser.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    Write-Host "Not elevated. Relaunching as Administrator..." -ForegroundColor Yellow
    $scriptPath = $MyInvocation.MyCommand.Path
    Start-Process -FilePath 'pwsh.exe' `
        -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$scriptPath`"", '-ExclusionPath', "`"$ExclusionPath`"") `
        -Verb RunAs
    exit
}

Write-Host "Running as Administrator." -ForegroundColor Green

try {
    $existing = (Get-MpPreference).ExclusionPath
    if ($existing -contains $ExclusionPath) {
        Write-Host "Exclusion already present: $ExclusionPath" -ForegroundColor Cyan
    } else {
        Add-MpPreference -ExclusionPath $ExclusionPath -ErrorAction Stop
        Write-Host "Added Defender exclusion: $ExclusionPath" -ForegroundColor Green
    }

    Write-Host ""
    Write-Host "Current exclusions:" -ForegroundColor Cyan
    (Get-MpPreference).ExclusionPath | ForEach-Object { Write-Host "  - $_" }
}
catch {
    Write-Host "Failed: $($_.Exception.Message)" -ForegroundColor Red
    Read-Host "Press Enter to close"
    exit 1
}

Write-Host ""
Read-Host "Done. Press Enter to close"
