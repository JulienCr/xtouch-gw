# XTouch GW v3 - PowerShell Build Script
# Usage: .\make.ps1 [command]
# Example: .\make.ps1 build

param(
    [Parameter(Position = 0)]
    [string]$Command = "help",
    
    [string]$Config = "config.example.yaml",
    [string]$LogLevel = "info",
    [string]$Module = "",
    [string]$TsDir = "D:\dev\xtouch-gw-v2"
)

# ANSI color codes
$Green = "`e[32m"
$Yellow = "`e[33m"
$Blue = "`e[34m"
$Red = "`e[31m"
$Reset = "`e[0m"

function Write-ColorOutput($Color, $Message) {
    Write-Host "${Color}${Message}${Reset}"
}

function Show-Help {
    Write-ColorOutput $Green "XTouch GW v3 - Rust Implementation"
    Write-Host ""
    Write-ColorOutput $Yellow "Usage: .\make.ps1 [command] [options]"
    Write-Host ""
    Write-ColorOutput $Blue "Development Commands:"
    Write-Host "  build         - Build debug version"
    Write-Host "  release       - Build optimized release version"
    Write-Host "  run           - Run with example config"
    Write-Host "  dev           - Run in watch mode (auto-rebuild)"
    Write-Host "  watch         - Watch for changes and check"
    Write-Host ""
    Write-ColorOutput $Blue "Testing Commands:"
    Write-Host "  test          - Run all tests"
    Write-Host "  test-verbose  - Run tests with output"
    Write-Host "  bench         - Run benchmarks"
    Write-Host "  check         - Type check without building"
    Write-Host "  clippy        - Run clippy linter"
    Write-Host "  fmt           - Format code"
    Write-Host ""
    Write-ColorOutput $Blue "TypeScript Reference:"
    Write-Host "  ts-run        - Run TypeScript version"
    Write-Host "  ts-test       - Run TypeScript tests"
    Write-Host "  ts-sniff      - Run TypeScript MIDI sniffer"
    Write-Host "  compare       - Instructions for side-by-side comparison"
    Write-Host ""
    Write-ColorOutput $Blue "Tools:"
    Write-Host "  sniffer       - Run MIDI sniffer (CLI)"
    Write-Host "  web-sniffer   - Run web MIDI sniffer"
    Write-Host "  docs          - Generate documentation"
    Write-Host "  clean         - Clean build artifacts"
    Write-Host ""
    Write-ColorOutput $Blue "Setup:"
    Write-Host "  setup         - Initial project setup"
    Write-Host "  install-deps  - Install Rust dependencies"
    Write-Host "  install-tools - Install development tools"
    Write-Host ""
    Write-ColorOutput $Blue "CI/Quality:"
    Write-Host "  ci            - Run CI checks (fmt, clippy, test)"
    Write-Host "  all           - Build everything and run tests"
    Write-Host ""
    Write-ColorOutput $Blue "Options:"
    Write-Host "  -Config       - Config file path (default: config.example.yaml)"
    Write-Host "  -LogLevel     - Log level (default: info)"
    Write-Host "  -Module       - Specific module for testing"
    Write-Host "  -TsDir        - TypeScript project directory"
    Write-Host ""
    Write-ColorOutput $Blue "Examples:"
    Write-Host "  .\make.ps1 build"
    Write-Host "  .\make.ps1 run -LogLevel debug"
    Write-Host "  .\make.ps1 test -Module midi"
}

function Invoke-Build {
    Write-ColorOutput $Yellow "Building debug version..."
    cargo build
    if ($LASTEXITCODE -eq 0) {
        Write-ColorOutput $Green "Build complete: target/debug/xtouch-gw.exe"
    }
}

function Invoke-Release {
    Write-ColorOutput $Yellow "Building release version..."
    cargo build --release
    if ($LASTEXITCODE -eq 0) {
        Write-ColorOutput $Green "Build complete: target/release/xtouch-gw.exe"
    }
}

function Invoke-Run {
    Invoke-Build
    Write-ColorOutput $Yellow "Running XTouch GW v3..."
    cargo run -- -c $Config --log-level $LogLevel
}

function Invoke-Dev {
    Write-ColorOutput $Yellow "Starting development mode (Ctrl+C to stop)..."
    cargo watch -x "run -- -c $Config --log-level debug"
}

function Invoke-Watch {
    Write-ColorOutput $Yellow "Watching for changes..."
    cargo watch -x check -x test
}

function Invoke-Test {
    Write-ColorOutput $Yellow "Running tests..."
    cargo test
    if ($LASTEXITCODE -eq 0) {
        Write-ColorOutput $Green "Tests complete!"
    }
}

function Invoke-TestVerbose {
    cargo test -- --nocapture --test-threads=1
}

function Invoke-Bench {
    Write-ColorOutput $Yellow "Running benchmarks..."
    cargo bench
}

function Invoke-Check {
    Write-ColorOutput $Yellow "Type checking..."
    cargo check
    if ($LASTEXITCODE -eq 0) {
        Write-ColorOutput $Green "Type check complete!"
    }
}

function Invoke-Clippy {
    Write-ColorOutput $Yellow "Running clippy..."
    cargo clippy -- -D warnings
    if ($LASTEXITCODE -eq 0) {
        Write-ColorOutput $Green "Clippy complete!"
    }
}

function Invoke-Fmt {
    Write-ColorOutput $Yellow "Formatting code..."
    cargo fmt
    Write-ColorOutput $Green "Formatting complete!"
}

function Invoke-FmtCheck {
    cargo fmt -- --check
}

function Invoke-Clean {
    Write-ColorOutput $Yellow "Cleaning build artifacts..."
    cargo clean
    if (Test-Path "logs\*.log") { Remove-Item "logs\*.log" -Force }
    if (Test-Path ".state\*.json") { Remove-Item ".state\*.json" -Force }
    Write-ColorOutput $Green "Clean complete!"
}

function Invoke-Docs {
    Write-ColorOutput $Yellow "Generating documentation..."
    cargo doc --no-deps --open
    Write-ColorOutput $Green "Documentation generated!"
}

function Invoke-Sniffer {
    Invoke-Build
    Write-ColorOutput $Yellow "Starting MIDI sniffer..."
    cargo run -- --sniffer
}

function Invoke-WebSniffer {
    Invoke-Build
    Write-ColorOutput $Yellow "Starting web MIDI sniffer on http://localhost:8123..."
    cargo run -- --web-sniffer --web-port 8123
}

function Invoke-TsRun {
    Write-ColorOutput $Yellow "Running TypeScript reference implementation..."
    Push-Location $TsDir
    pnpm dev
    Pop-Location
}

function Invoke-TsTest {
    Write-ColorOutput $Yellow "Running TypeScript tests..."
    Push-Location $TsDir
    pnpm test
    Pop-Location
}

function Invoke-TsSniff {
    Write-ColorOutput $Yellow "Starting TypeScript MIDI sniffer..."
    Push-Location $TsDir
    pnpm sniff:web
    Pop-Location
}

function Invoke-TsBuild {
    Write-ColorOutput $Yellow "Building TypeScript version..."
    Push-Location $TsDir
    pnpm build
    Pop-Location
}

function Invoke-Compare {
    Write-ColorOutput $Green "========================================"
    Write-ColorOutput $Green " Running Both Versions Side-by-Side"
    Write-ColorOutput $Green "========================================"
    Write-Host ""
    Write-ColorOutput $Yellow "Open 4 terminals and run:"
    Write-Host "  Terminal 1: .\make.ps1 ts-run"
    Write-Host "  Terminal 2: .\make.ps1 run"
    Write-Host "  Terminal 3: .\make.ps1 ts-sniff"
    Write-Host "  Terminal 4: .\make.ps1 web-sniffer"
    Write-Host ""
    Write-ColorOutput $Blue "Starting Rust version in this terminal..."
    cargo run -- -c $Config --log-level $LogLevel
}

function Invoke-Setup {
    Write-ColorOutput $Green "Setting up XTouch GW v3..."
    
    # Create directories
    @("target", ".state", "logs") | ForEach-Object {
        if (!(Test-Path $_)) {
            New-Item -ItemType Directory -Path $_ | Out-Null
            Write-Host "Created directory: $_"
        }
    }
    
    # Copy config if needed
    if (!(Test-Path "config.yaml")) {
        Copy-Item "config.example.yaml" "config.yaml"
        Write-Host "Created config.yaml from example"
    }
    
    Write-ColorOutput $Green "Installing Rust dependencies..."
    cargo fetch
    Write-ColorOutput $Green "Setup complete!"
}

function Invoke-InstallDeps {
    Write-ColorOutput $Yellow "Fetching dependencies..."
    cargo fetch
    cargo update
    Write-ColorOutput $Green "Dependencies updated!"
}

function Invoke-InstallTools {
    Write-ColorOutput $Yellow "Installing development tools..."
    cargo install cargo-watch
    cargo install cargo-edit
    cargo install cargo-audit
    cargo install cargo-outdated
    cargo install flamegraph
    Write-ColorOutput $Green "Tools installed!"
}

function Invoke-CI {
    Write-ColorOutput $Yellow "Running CI checks..."
    
    Invoke-FmtCheck
    if ($LASTEXITCODE -ne 0) { 
        Write-ColorOutput $Red "Format check failed!"
        exit 1 
    }
    
    Invoke-Clippy
    if ($LASTEXITCODE -ne 0) { 
        Write-ColorOutput $Red "Clippy check failed!"
        exit 1 
    }
    
    Invoke-Test
    if ($LASTEXITCODE -ne 0) { 
        Write-ColorOutput $Red "Tests failed!"
        exit 1 
    }
    
    Write-ColorOutput $Green "All CI checks passed!"
}

function Invoke-All {
    Invoke-Clean
    Invoke-Build
    Invoke-Release
    Invoke-Test
    Invoke-Docs
    Write-ColorOutput $Green "Full build complete!"
}

function Invoke-Info {
    Write-ColorOutput $Blue "Environment Information:"
    Write-Host ""
    Write-Host "Rust version:"
    rustc --version
    Write-Host ""
    Write-Host "Cargo version:"
    cargo --version
    Write-Host ""
    Write-Host "Project root: $(Get-Location)"
    Write-Host "TypeScript dir: $TsDir"
    Write-Host ""
    Write-Host "Config file: $Config"
    Write-Host "Log level: $LogLevel"
}

function Invoke-Validate {
    Write-ColorOutput $Yellow "Validation Steps:"
    Write-Host "1. Start TypeScript version: .\make.ps1 ts-run"
    Write-Host "2. Capture MIDI sequence"
    Write-Host "3. Start Rust version: .\make.ps1 run"
    Write-Host "4. Compare MIDI outputs"
    Write-Host "5. Check state consistency"
    Write-Host ""
    Write-Host "See DEVELOPMENT.md for detailed validation process."
}

# Main command dispatcher
switch ($Command.ToLower()) {
    "help" { Show-Help }
    "build" { Invoke-Build }
    "release" { Invoke-Release }
    "run" { Invoke-Run }
    "dev" { Invoke-Dev }
    "watch" { Invoke-Watch }
    "test" { Invoke-Test }
    "test-verbose" { Invoke-TestVerbose }
    "bench" { Invoke-Bench }
    "check" { Invoke-Check }
    "clippy" { Invoke-Clippy }
    "fmt" { Invoke-Fmt }
    "fmt-check" { Invoke-FmtCheck }
    "clean" { Invoke-Clean }
    "docs" { Invoke-Docs }
    "sniffer" { Invoke-Sniffer }
    "web-sniffer" { Invoke-WebSniffer }
    "ts-run" { Invoke-TsRun }
    "ts-test" { Invoke-TsTest }
    "ts-sniff" { Invoke-TsSniff }
    "ts-build" { Invoke-TsBuild }
    "compare" { Invoke-Compare }
    "setup" { Invoke-Setup }
    "install-deps" { Invoke-InstallDeps }
    "install-tools" { Invoke-InstallTools }
    "ci" { Invoke-CI }
    "all" { Invoke-All }
    "info" { Invoke-Info }
    "validate" { Invoke-Validate }
    
    # Shortcuts
    "b" { Invoke-Build }
    "r" { Invoke-Run }
    "t" { Invoke-Test }
    "c" { Invoke-Check }
    "f" { Invoke-Fmt }
    "cl" { Invoke-Clean }
    
    default {
        Write-ColorOutput $Red "Unknown command: $Command"
        Write-Host ""
        Show-Help
        exit 1
    }
}
