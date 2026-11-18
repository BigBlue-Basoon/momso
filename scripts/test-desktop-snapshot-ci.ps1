# Run in a new PowerShell process with an existing snapshot commit and a new,
# short absolute copy path. Add -IncludeInstaller to build and checksum NSIS.
# The current Rust toolchain and Cargo registry cache are reused; source files,
# npm cache, user-data environment, credential-store tests, and target are isolated.
param(
    [Parameter(Mandatory = $true)] [string] $SnapshotRepository,
    [Parameter(Mandatory = $true)] [string] $SnapshotCommit,
    [Parameter(Mandatory = $true)] [string] $CopyPath,
    [switch] $IncludeInstaller
)

$ErrorActionPreference = 'Stop'
if ($SnapshotCommit -notmatch '^[0-9a-fA-F]{7,40}$') { throw 'Use a snapshot commit hash.' }
if (-not [System.IO.Path]::IsPathRooted($CopyPath)) { throw 'Use an absolute copy path.' }
$copy = [System.IO.Path]::GetFullPath($CopyPath)
$repo = (Resolve-Path -LiteralPath $SnapshotRepository).Path
if (Test-Path -LiteralPath $copy) { throw 'The copy path already exists.' }
$os = Get-CimInstance Win32_OperatingSystem
$disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
if ($os.FreePhysicalMemory / 1MB -lt 4 -or $disk.FreeSpace / 1GB -lt 60) { throw 'Insufficient available memory or disk space.' }
$git = (Get-Command git -ErrorAction Stop).Source
$python = (Get-Command python -ErrorAction Stop).Source
$cargo = (Get-Command cargo -ErrorAction Stop).Source
$npm = (Get-Command npm.cmd -ErrorAction Stop).Source
$node = (Get-Command node -ErrorAction Stop).Source
$nodeVersion = & $node --version
if ($LASTEXITCODE -ne 0 -or $nodeVersion -notmatch '^v24\.') { throw 'Node.js 24 is required by the desktop CI workflow.' }
$originalProfile = $env:USERPROFILE
$cargoCache = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $originalProfile '.cargo' }
$rustupCache = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $originalProfile '.rustup' }
$sha = & $git --git-dir=$repo rev-parse --verify "$SnapshotCommit^{commit}"
if ($LASTEXITCODE -ne 0 -or $sha -notmatch '^[0-9a-f]{40}$') { throw 'Snapshot commit not found.' }
New-Item -ItemType Directory -Path $copy | Out-Null
$archive = Join-Path $copy 'source.tar'
& $git --git-dir=$repo archive --format=tar --output=$archive $sha
if ($LASTEXITCODE -ne 0) { throw 'Cannot archive the snapshot.' }
& $python -c 'import sys,tarfile;tarfile.open(sys.argv[1]).extractall(sys.argv[2],filter=tarfile.data_filter)' $archive $copy
if ($LASTEXITCODE -ne 0) { throw 'Cannot extract the snapshot.' }
Remove-Item -LiteralPath $archive
$credentials = Join-Path $copy 'apps/alpha/desktop/src-tauri/src/credentials.rs'
if (-not (Select-String -LiteralPath $credentials -SimpleMatch 'MOMSO_TEST_FAKE_CREDENTIAL_STORE' -Quiet)) {
    throw 'Snapshot lacks the isolated test credential store; refusing to access the real store.'
}
$profile = Join-Path $copy 'isolated-user'
$temp = Join-Path $copy 'isolated-temp'
if ((Test-Path -LiteralPath $profile) -or (Test-Path -LiteralPath $temp)) {
    throw 'Snapshot contains an isolation directory; refusing to reuse it.'
}
foreach ($path in @($profile, $temp, (Join-Path $profile 'AppData/Roaming'), (Join-Path $profile 'AppData/Local'))) {
    New-Item -ItemType Directory -Path $path -Force | Out-Null
}
$env:USERPROFILE = $profile
$env:APPDATA = Join-Path $profile 'AppData/Roaming'
$env:LOCALAPPDATA = Join-Path $profile 'AppData/Local'
$env:TEMP = $temp
$env:TMP = $temp
$env:GIT_CONFIG_NOSYSTEM = '1'
$env:GIT_CONFIG_GLOBAL = Join-Path $profile 'empty-gitconfig'
$env:CARGO_HOME = $cargoCache
$env:RUSTUP_HOME = $rustupCache
$env:CARGO_TARGET_DIR = Join-Path $copy 'apps/alpha/desktop/src-tauri/target'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_BUILD_JOBS = '1'
$env:MOMSO_TEST_FAKE_CREDENTIAL_STORE = '1'
$env:GITHUB_SHA = $sha
$env:GITHUB_WORKSPACE = $copy
$env:npm_config_userconfig = Join-Path $profile 'empty-npmrc'
$env:npm_config_cache = Join-Path $copy 'npm-cache'
$env:PATH = (($env:PATH -split ';') | Where-Object {
    $directory = $_
    if (-not $directory) { return $false }
    foreach ($tool in @('codex', 'claude', 'agy')) {
        foreach ($extension in @('.exe', '.cmd', '.bat', '.ps1', '')) {
            if (Test-Path -LiteralPath (Join-Path $directory ($tool + $extension))) { return $false }
        }
    }
    return $true
}) -join ';'
foreach ($tool in @('codex', 'claude', 'agy')) {
    if (Get-Command $tool -CommandType Application, ExternalScript -ErrorAction SilentlyContinue) {
        throw "A provider CLI is still accessible: $tool"
    }
}
function Run-Step([string] $name, [string] $program, [string[]] $arguments) {
    Write-Host "Running: $name"
    if ($program -eq $cargo -or $name -eq 'Windows installer') {
        $currentOs = Get-CimInstance Win32_OperatingSystem
        $currentDisk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
        if ($currentOs.FreePhysicalMemory / 1MB -lt 4 -or $currentDisk.FreeSpace / 1GB -lt 60) {
            throw 'Insufficient available memory or disk space before cargo.'
        }
    }
    & $program @arguments
    if ($LASTEXITCODE -ne 0) { throw "$name failed with exit code $LASTEXITCODE." }
}
Write-Host "Snapshot: $sha"
Write-Host "Isolated copy: $copy"
Push-Location $copy
try {
    Run-Step 'public hygiene' $python @('-B', 'scripts/check-public-hygiene.py')
    Run-Step 'npm ci' $npm @('ci')
    Run-Step 'production dependency audit' $npm @('audit', '--omit=dev', '--audit-level=high')
    Run-Step 'desktop version' $npm @('run', 'desktop:version:check')
    Run-Step 'artifact metadata tests' $npm @('run', 'desktop:artifact:metadata:test')
    Run-Step 'desktop tests' $npm @('test', '--workspace', '@momso/alpha-desktop')
    Run-Step 'desktop webview build' $npm @('run', 'desktop:ui:build')
    Run-Step 'Rust format' $cargo @('fmt', '--manifest-path', 'apps/alpha/desktop/src-tauri/Cargo.toml', '--', '--check')
    Run-Step 'prepare Node' $node @('apps/alpha/desktop/scripts/prepare-node.mjs')
    Run-Step 'Rust tests' $cargo @('test', '--manifest-path', 'apps/alpha/desktop/src-tauri/Cargo.toml', '-j', '1', '--', '--test-threads=2')
    Run-Step 'Rust clippy' $cargo @('clippy', '--manifest-path', 'apps/alpha/desktop/src-tauri/Cargo.toml', '--all-targets', '-j', '1', '--', '-D', 'warnings')
    if ($IncludeInstaller) {
        $bundle = [System.IO.Path]::GetFullPath((Join-Path $env:CARGO_TARGET_DIR 'release/bundle'))
        $prefix = $copy.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
        if (-not $bundle.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Bundle is outside the copy.' }
        if (Test-Path -LiteralPath $bundle) { Remove-Item -LiteralPath $bundle -Recurse -Force }
        Run-Step 'Windows installer' $npm @('run', 'desktop:build')
        Run-Step 'artifact checksums' $npm @('run', 'desktop:artifact:metadata')
    } else {
        Write-Host 'Installer and artifact checksums skipped; use -IncludeInstaller to run them.'
    }
} finally { Pop-Location }
