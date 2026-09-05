# Build the CLI for the exact target being packaged and expose it using the
# target-triple filename required by Tauri's externalBin configuration.
param(
    [string]$Target = ""
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Target)) {
    $Target = (rustc --print host-tuple).Trim()
}
if ([string]::IsNullOrWhiteSpace($Target)) {
    throw "Unable to determine the Rust target triple"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot
cargo build --release --target $Target --locked

$sidecarDir = Join-Path $repoRoot "src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $sidecarDir | Out-Null
$source = Join-Path $repoRoot "target\$Target\release\codexctl.exe"
$destination = Join-Path $sidecarDir "codexctl-$Target.exe"
Copy-Item -Force $source $destination
Write-Host "Prepared $destination"
