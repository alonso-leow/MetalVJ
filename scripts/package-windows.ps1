# package-windows.ps1 -- cut a Windows release artifact for MetalVJ.
#
#   powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1
#
# Produces: dist/MetalVJ-<version>-windows-x86_64.exe
# and prints its sha256 (the updater verifies the download against it).
#
# <version> is read from [workspace.package] version in Cargo.toml -- never
# hardcoded here. The same string names the artifacts on every platform, so
# the CI release job and scripts/package-linux.sh stay in step.
#
# Why a script and not `cargo dist`: the repo has one binary and one target
# per platform; cargo-dist would add a second source of truth for names that
# the updater already relies on.
#
# Icon / version resource: deliberately NOT embedded. Cargo.lock has neither
# `winres` nor `embed-resource` (checked), and adding a build-dependency to
# metal-dash to stamp a resource is a build-time cost and a lockfile change
# for a placeholder icon. The .ico is produced beside the .exe instead and
# docs/DISTRIBUTION.md explains how to wire it in later. Say the word and the
# build script can shell out to `mt.exe` + `rc.exe` from the MSVC Build Tools
# if they are on PATH -- but that is opt-in, not a silent dependency.

[CmdletBinding()]
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$Cargo = "cargo"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
Set-Location -LiteralPath $Root

function Fail([string]$msg) {
    Write-Host "package-windows: $msg" -ForegroundColor Red
    exit 1
}

# --- toolchain -------------------------------------------------------------
# Fail loudly and specifically: a missing cargo should not look like a build
# error four minutes from now.
if (-not (Get-Command $Cargo -CommandType Application -ErrorAction SilentlyContinue)) {
    Fail "cargo not found on PATH. Install Rust (https://rustup.rs) and re-open the shell."
}
$cargoVersion = (& $Cargo --version 2>&1 | Out-String).Trim()
Write-Host "package-windows: $cargoVersion"

if (-not (Get-Command rustc -CommandType Application -ErrorAction SilentlyContinue)) {
    Fail "rustc not found on PATH (cargo is present but the toolchain is incomplete)."
}

# link.exe comes from the MSVC Build Tools; cargo finds it only inside a
# Developer shell or when rustup has the msvc toolchain wired up. Probe rather
# than discover this after the whole dependency graph compiles.
if ($Target -like "*-msvc" -and -not (Get-Command link.exe -CommandType Application -ErrorAction SilentlyContinue)) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $vsOk = $false
    if (Test-Path -LiteralPath $vswhere) {
        $vsPath = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null)
        if ($vsPath) { $vsOk = $true }
    }
    if (-not $vsOk) {
        Fail "no MSVC linker (link.exe): install the 'Desktop development with C++' workload (Visual Studio Build Tools)."
    }
    Write-Host "package-windows: link.exe not on PATH but MSVC Build Tools found; cargo/rustc will locate it."
}

# --- version ---------------------------------------------------------------
$cargoToml = Join-Path $Root "Cargo.toml"
$toml = Get-Content -LiteralPath $cargoToml -Raw
# Read only the [workspace.package] table so a dependency's version is never
# mistaken for ours.
$m = [regex]::Match($toml, '(?ms)^\[workspace\.package\]\s*$(?<body>.*?)(?=^\[|\z)')
if (-not $m.Success) { Fail "no [workspace.package] table in Cargo.toml" }
$v = [regex]::Match($m.Groups['body'].Value, '(?m)^\s*version\s*=\s*"([^"]+)"')
if (-not $v.Success) { Fail "no version in [workspace.package] of Cargo.toml" }
$Version = $v.Groups[1].Value
if ($Version -notmatch '^\d+\.\d+\.\d+([-+].*)?$') {
    Fail "workspace version '$Version' is not a semver triple; the artifact name would be ambiguous"
}
Write-Host "package-windows: version $Version"

$Name = "MetalVJ-$Version-windows-x86_64.exe"
$Dist = Join-Path $Root "dist"
if (-not (Test-Path -LiteralPath $Dist)) {
    New-Item -ItemType Directory -Force -Path $Dist | Out-Null
}
$OutPath = Join-Path $Dist $Name
# Idempotent: a stale artifact must not survive a failed rebuild and get
# hashed as if it were this run's output.
if (Test-Path -LiteralPath $OutPath) { Remove-Item -LiteralPath $OutPath -Force }

# --- build -----------------------------------------------------------------
$targetArgs = @()
if ($Target -ne "x86_64-pc-windows-msvc") {
    $targetArgs = @("--target", $Target)
    $installed = (& rustup target list --installed 2>$null) -split "`r?`n"
    if (-not ($installed -contains $Target)) {
        Fail "rust target $Target not installed: rustup target add $Target"
    }
}
$cargoArgs = @("build", "-p", "metal-dash", "--release") + $targetArgs
Write-Host "package-windows: $Cargo $($cargoArgs -join ' ')"
# Windows PowerShell 5.1 turns a native command's stderr into a terminating
# error when $ErrorActionPreference is 'Stop'. GitHub's `powershell` step
# wrapper literally prepends `$ErrorActionPreference = 'stop'` (see
# ScriptHandlerHelpers.FixUpScriptContents in actions/runner), so without this
# the build would abort on cargo's first stderr warning. Drop to 'Continue'
# for the call and treat $LASTEXITCODE as the only verdict -- the same shape
# cargo's own docs recommend. Restored afterwards so the rest of the script
# keeps its strict (and `Set-StrictMode`) behaviour.
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $Cargo @cargoArgs
$cargoExit = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($cargoExit -ne 0) { Fail "cargo build failed (exit $cargoExit)" }

$targetDir = $env:CARGO_TARGET_DIR
if (-not $targetDir) { $targetDir = Join-Path $Root "target" }
if ($targetArgs.Count -gt 0) {
    $Bin = Join-Path $targetDir (Join-Path $Target "release/metal-dash.exe")
} else {
    $Bin = Join-Path $targetDir "release/metal-dash.exe"
}
if (-not (Test-Path -LiteralPath $Bin)) { Fail "build succeeded but $Bin is missing" }

# --- assemble --------------------------------------------------------------
Copy-Item -LiteralPath $Bin -Destination $OutPath -Force

# The AppImage's icon travels with the Windows artifact too, plus the .ico
# that *would* be embedded. Both are placeholders (see packaging/make-icon.ps1).
$png = Join-Path $Root "packaging/icons/MetalVJ.png"
if (Test-Path -LiteralPath $png) {
    Copy-Item -LiteralPath $png -Destination (Join-Path $Dist "MetalVJ.png") -Force
} else {
    Write-Host "package-windows: packaging/icons/MetalVJ.png missing; run packaging/make-icon.ps1" -ForegroundColor Yellow
}

# --- report ----------------------------------------------------------------
$item = Get-Item -LiteralPath $OutPath
$hash = (Get-FileHash -LiteralPath $OutPath -Algorithm SHA256).Hash.ToLowerInvariant()
$sizeText = if ($item.Length -ge 1MB) {
    "{0:N1} MB" -f ($item.Length / 1MB)
} else {
    "{0:N0} KB" -f ($item.Length / 1KB)
}
Write-Host ""
Write-Host "artifact : $($item.FullName)"
Write-Host "size     : $($item.Length) bytes ($sizeText)"
Write-Host "sha256   : $hash"
Write-Host ""
# Machine-readable line last: CI greps for it, humans read the block above.
Write-Host "SHA256 $hash  $Name"
