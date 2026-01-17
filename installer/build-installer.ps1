# XTouch GW - Installer Build Script
# Usage: .\installer\build-installer.ps1
# Requires: Inno Setup 6+ installed

param(
    [switch]$SkipBuild,
    [switch]$SkipStreamDeck
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

# Project root (one level up from installer/)
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$InstallerDir = $PSScriptRoot

# Paths
$IssFile = Join-Path $InstallerDir "xtouch-gw.iss"
$DistDir = Join-Path $ProjectRoot "dist"
$ReleaseExe = Join-Path $ProjectRoot "target\release\xtouch-gw.exe"
$VersionFile = Join-Path $ProjectRoot "VERSION.txt"
$StreamDeckPlugin = Join-Path $ProjectRoot "streamdeck-plugin\com.juliencr.xtouch-gw.sdPlugin"

# Find Inno Setup compiler
function Find-InnoSetup {
    $commonPaths = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles}\Inno Setup 6\ISCC.exe",
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe"
    )

    foreach ($path in $commonPaths) {
        if (Test-Path $path) {
            return $path
        }
    }

    # Try to find in PATH
    $iscc = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue
    if ($iscc) {
        return $iscc.Source
    }

    return $null
}

function Get-Version {
    if (Test-Path $VersionFile) {
        $lines = Get-Content $VersionFile
        return $lines[0].Trim()
    }
    return "3.0.0"
}

# Main script
Write-ColorOutput $Cyan "========================================"
Write-ColorOutput $Cyan " XTouch GW - Installer Builder"
Write-ColorOutput $Cyan "========================================"
Write-Host ""

# Check for Inno Setup
$iscc = Find-InnoSetup
if (-not $iscc) {
    Write-ColorOutput $Red "ERROR: Inno Setup 6 not found!"
    Write-Host ""
    Write-Host "Please install Inno Setup 6 from:"
    Write-Host "  https://jrsoftware.org/isinfo.php"
    Write-Host ""
    Write-Host "Or add ISCC.exe to your PATH."
    exit 1
}

Write-ColorOutput $Blue "Found Inno Setup: $iscc"

# Get version
$version = Get-Version
Write-ColorOutput $Blue "Building installer for version: $version"
Write-Host ""

# Build release binary if needed
if (-not $SkipBuild) {
    Write-ColorOutput $Yellow "Building release binary..."
    Push-Location $ProjectRoot
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        Write-ColorOutput $Red "Build failed!"
        Pop-Location
        exit 1
    }
    Pop-Location
    Write-ColorOutput $Green "Build complete!"
} else {
    Write-ColorOutput $Yellow "Skipping build (--SkipBuild)"
}

# Verify release binary exists
if (-not (Test-Path $ReleaseExe)) {
    Write-ColorOutput $Red "ERROR: Release binary not found at $ReleaseExe"
    Write-Host "Run without -SkipBuild to build the release binary."
    exit 1
}

# Build Stream Deck plugin if needed
if (-not $SkipStreamDeck) {
    $pluginPackageJson = Join-Path $StreamDeckPlugin "package.json"
    if (Test-Path $pluginPackageJson) {
        Write-ColorOutput $Yellow "Building Stream Deck plugin..."
        Push-Location $StreamDeckPlugin
        pnpm install --frozen-lockfile 2>$null
        pnpm build
        if ($LASTEXITCODE -ne 0) {
            Write-ColorOutput $Red "Stream Deck plugin build failed!"
            Pop-Location
            exit 1
        }
        Pop-Location
        Write-ColorOutput $Green "Stream Deck plugin built!"
    } else {
        Write-ColorOutput $Yellow "Stream Deck plugin not found, skipping..."
    }
} else {
    Write-ColorOutput $Yellow "Skipping Stream Deck build (--SkipStreamDeck)"
}

# Create dist directory
if (-not (Test-Path $DistDir)) {
    New-Item -ItemType Directory -Path $DistDir | Out-Null
    Write-Host "Created dist directory"
}

# Compile installer
Write-Host ""
Write-ColorOutput $Yellow "Compiling installer..."
& $iscc "/DMyAppVersion=$version" $IssFile

if ($LASTEXITCODE -ne 0) {
    Write-ColorOutput $Red "Installer compilation failed!"
    exit 1
}

# Find output file
$outputFile = Join-Path $DistDir "xtouch-gw-$version-setup.exe"
if (Test-Path $outputFile) {
    $size = [math]::Round((Get-Item $outputFile).Length / 1MB, 2)
    Write-Host ""
    Write-ColorOutput $Green "========================================"
    Write-ColorOutput $Green " Installer built successfully!"
    Write-ColorOutput $Green "========================================"
    Write-Host ""
    Write-Host "Output: $outputFile"
    Write-Host "Size: $size MB"
} else {
    Write-ColorOutput $Yellow "Installer created (check dist/ folder)"
}
