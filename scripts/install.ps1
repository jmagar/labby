<#
.SYNOPSIS
    Install labby — the Lab homelab control plane binary — on Windows.

.DESCRIPTION
    Downloads the latest GitHub release archive for x86_64 Windows, verifies
    its SHA-256, and installs labby.exe into %LOCALAPPDATA%\labby\bin (added to
    the user PATH). Falls back to `cargo install --git` when no release asset
    exists and a Rust toolchain is present.

    This script's ONLY job is bootstrap: getting labby onto PATH. Everything
    after that is owned by the binary — run `labby setup` for the first-run
    flow (config, credentials, connectivity checks).

.PARAMETER InstallDir
    Install directory. Default: $env:LOCALAPPDATA\labby\bin
    (or set $env:LAB_INSTALL_DIR).

.PARAMETER Version
    Release tag, e.g. v0.23.0. Default: latest (or $env:LAB_INSTALL_VERSION).

.EXAMPLE
    irm https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.ps1 | iex
#>
[CmdletBinding()]
param(
    [string]$InstallDir = $(if ($env:LAB_INSTALL_DIR) { $env:LAB_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'labby\bin' }),
    [string]$Version    = $(if ($env:LAB_INSTALL_VERSION) { $env:LAB_INSTALL_VERSION } else { 'latest' }),
    [string]$Repo       = $(if ($env:LAB_INSTALL_REPO) { $env:LAB_INSTALL_REPO } else { 'jmagar/lab' })
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Info($msg) { Write-Host $msg -ForegroundColor Cyan }
function Fail($msg) { Write-Error "install.ps1: $msg"; exit 1 }

$arch = (Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture
# 9 = x64. ARM64 (12) has no prebuilt asset; fall through to cargo.
$asset = 'lab-x86_64-pc-windows-msvc.zip'

function Install-FromRelease {
    if ($arch -ne 9) { return $false }
    $base = if ($Version -eq 'latest') {
        "https://github.com/$Repo/releases/latest/download"
    } else {
        "https://github.com/$Repo/releases/download/$Version"
    }

    $tmp = Join-Path $env:TEMP ("labby-install-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        Write-Info "downloading $base/$asset ..."
        $zip = Join-Path $tmp $asset
        try {
            Invoke-WebRequest -Uri "$base/$asset" -OutFile $zip -UseBasicParsing
        } catch {
            return $false
        }

        # SHA-256 verification when the checksum asset is published.
        try {
            $shaFile = "$zip.sha256"
            Invoke-WebRequest -Uri "$base/$asset.sha256" -OutFile $shaFile -UseBasicParsing
            $expected = ((Get-Content $shaFile -Raw) -split '\s+')[0].Trim().ToLower()
            $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
            if ($expected -ne $actual) { Fail "checksum verification FAILED for $asset (expected $expected, got $actual)" }
            Write-Info "sha256 verified"
        } catch {
            Write-Warning "no .sha256 asset published — skipping checksum verification"
        }

        Expand-Archive -Path $zip -DestinationPath $tmp -Force
        $bin = Get-ChildItem -Path $tmp -Recurse -Filter 'labby.exe' | Select-Object -First 1
        if (-not $bin) { Fail "archive $asset did not contain labby.exe" }

        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        Copy-Item -Path $bin.FullName -Destination (Join-Path $InstallDir 'labby.exe') -Force
        return $true
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}

function Install-FromSource {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { return $false }
    Write-Info "no release asset available — building from source (this takes a while) ..."
    & cargo install --git "https://github.com/$Repo" --bin labby --all-features --root (Split-Path $InstallDir -Parent)
    return $LASTEXITCODE -eq 0
}

function Add-ToUserPath($dir) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $dir) {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
        Write-Info "added $dir to your user PATH (restart the shell to pick it up)"
    }
}

if (-not (Install-FromRelease)) {
    if (-not (Install-FromSource)) {
        Fail @"
could not install: no prebuilt release for this Windows arch and no cargo toolchain found.
Install a Rust toolchain (https://rustup.rs) and re-run, or build from a clone:
  git clone https://github.com/$Repo; cd lab; cargo install --path crates/labby --bin labby --all-features
"@
    }
}

Add-ToUserPath $InstallDir
$exe = Join-Path $InstallDir 'labby.exe'
$ver = if (Test-Path $exe) { & $exe --version } else { $exe }
Write-Host ""
Write-Info "labby installed: $ver"
Write-Info "next: run 'labby setup' to start the first-run flow"
