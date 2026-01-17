# XTouch GW - Version Bump Script
# Usage: .\scripts\version-bump.ps1
# This script interactively bumps version numbers across all project files

param(
    [ValidateSet("major", "minor", "patch", "")]
    [string]$BumpType = ""
)

$ErrorActionPreference = "Stop"

# ANSI color codes
$Green = "`e[32m"
$Yellow = "`e[33m"
$Blue = "`e[34m"
$Red = "`e[31m"
$Cyan = "`e[36m"
$Reset = "`e[0m"

function Write-ColorOutput($Color, $Message) {
    Write-Host "${Color}${Message}${Reset}"
}

# Project root (one level up from scripts/)
$ProjectRoot = Split-Path -Parent $PSScriptRoot

# Files to update
$CargoToml = Join-Path $ProjectRoot "Cargo.toml"
$VersionTxt = Join-Path $ProjectRoot "VERSION.txt"
$ManifestJson = Join-Path $ProjectRoot "streamdeck-plugin\com.juliencr.xtouch-gw.sdPlugin\manifest.json"
$PackageJson = Join-Path $ProjectRoot "streamdeck-plugin\com.juliencr.xtouch-gw.sdPlugin\package.json"

function Get-CurrentVersion {
    # Read version from Cargo.toml
    $cargoContent = Get-Content $CargoToml -Raw
    if ($cargoContent -match 'version\s*=\s*"(\d+\.\d+\.\d+)"') {
        return $Matches[1]
    }
    throw "Could not find version in Cargo.toml"
}

function Parse-Version([string]$Version) {
    $parts = $Version.Split('.')
    return @{
        Major = [int]$parts[0]
        Minor = [int]$parts[1]
        Patch = [int]$parts[2]
    }
}

function Format-Version($VersionParts) {
    return "$($VersionParts.Major).$($VersionParts.Minor).$($VersionParts.Patch)"
}

function Bump-Version([string]$CurrentVersion, [string]$Type) {
    $parts = Parse-Version $CurrentVersion
    switch ($Type) {
        "major" {
            $parts.Major++
            $parts.Minor = 0
            $parts.Patch = 0
        }
        "minor" {
            $parts.Minor++
            $parts.Patch = 0
        }
        "patch" {
            $parts.Patch++
        }
    }
    return Format-Version $parts
}

function Update-CargoToml([string]$NewVersion) {
    $content = Get-Content $CargoToml -Raw
    $content = $content -replace '(version\s*=\s*")(\d+\.\d+\.\d+)(")', "`${1}$NewVersion`${3}"
    Set-Content $CargoToml $content -NoNewline
    Write-Host "  Updated: Cargo.toml"
}

function Update-VersionTxt([string]$NewVersion) {
    $today = Get-Date -Format "yyyy-MM-dd"
    $content = "$NewVersion`n$today`n"
    Set-Content $VersionTxt $content -NoNewline
    Write-Host "  Updated: VERSION.txt ($NewVersion, $today)"
}

function Update-ManifestJson([string]$NewVersion) {
    if (Test-Path $ManifestJson) {
        $json = Get-Content $ManifestJson -Raw | ConvertFrom-Json
        $json.Version = $NewVersion
        $json | ConvertTo-Json -Depth 10 | Set-Content $ManifestJson
        Write-Host "  Updated: manifest.json (Stream Deck plugin)"
    } else {
        Write-Host "  Skipped: manifest.json (file not found)"
    }
}

function Update-PackageJson([string]$NewVersion) {
    if (Test-Path $PackageJson) {
        $json = Get-Content $PackageJson -Raw | ConvertFrom-Json
        $json.version = $NewVersion
        $json | ConvertTo-Json -Depth 10 | Set-Content $PackageJson
        Write-Host "  Updated: package.json (Stream Deck plugin)"
    } else {
        Write-Host "  Skipped: package.json (file not found)"
    }
}

# Main script
Write-ColorOutput $Cyan "========================================"
Write-ColorOutput $Cyan " XTouch GW - Version Bump"
Write-ColorOutput $Cyan "========================================"
Write-Host ""

# Get current version
$currentVersion = Get-CurrentVersion
Write-ColorOutput $Blue "Current version: $currentVersion"
Write-Host ""

# Ask for bump type if not provided
if (-not $BumpType) {
    Write-ColorOutput $Yellow "Select release type:"
    Write-Host "  [1] Major (breaking changes)"
    Write-Host "  [2] Minor (new features)"
    Write-Host "  [3] Patch (bug fixes)"
    Write-Host ""

    $choice = Read-Host "Enter choice (1-3)"

    switch ($choice) {
        "1" { $BumpType = "major" }
        "2" { $BumpType = "minor" }
        "3" { $BumpType = "patch" }
        default {
            Write-ColorOutput $Red "Invalid choice. Exiting."
            exit 1
        }
    }
}

# Calculate new version
$newVersion = Bump-Version $currentVersion $BumpType
$today = Get-Date -Format "yyyy-MM-dd"

Write-Host ""
Write-ColorOutput $Yellow "Version change: $currentVersion -> $newVersion"
Write-ColorOutput $Yellow "Release date: $today"
Write-Host ""

# Confirm
$confirm = Read-Host "Proceed with version bump? (y/n)"
if ($confirm -ne "y" -and $confirm -ne "Y") {
    Write-ColorOutput $Yellow "Cancelled."
    exit 0
}

Write-Host ""
Write-ColorOutput $Blue "Updating files..."

# Update all files
Update-CargoToml $newVersion
Update-VersionTxt $newVersion
Update-ManifestJson $newVersion
Update-PackageJson $newVersion

Write-Host ""
Write-ColorOutput $Green "Version bump complete!"
Write-Host ""
Write-ColorOutput $Yellow "Next steps:"
Write-Host "  1. Review changes: git diff"
Write-Host "  2. Commit: git add -A && git commit -m `"Release v$newVersion`""
Write-Host "  3. Build installer: .\make.ps1 package"
Write-Host "  4. Tag and push: git tag v$newVersion && git push --tags"
