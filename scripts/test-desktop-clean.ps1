param(
    [Parameter(Mandatory = $true)]
    [string] $CopyPath
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$copy = [System.IO.Path]::GetFullPath($CopyPath)
if (Test-Path -LiteralPath $copy) { throw 'The clean-copy path already exists.' }
if (-not [System.IO.Path]::IsPathRooted($CopyPath)) { throw 'Use an absolute clean-copy path.' }
$ram = (Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB
$disk = (Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'").FreeSpace / 1GB
if ($ram -lt 4 -or $disk -lt 60) { throw 'Insufficient available memory or disk space.' }
$cargo = (Get-Command cargo -ErrorAction Stop).Source
$git = (Get-Command git -ErrorAction Stop).Source
$python = (Get-Command python -ErrorAction Stop).Source
$originalProfile = $env:USERPROFILE
$cargoCache = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $originalProfile '.cargo' }
$rustupCache = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $originalProfile '.rustup' }
Push-Location $root
try {
    & $git diff --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Commit or preserve workspace changes before testing HEAD.' }
    & $git diff --cached --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Commit or preserve staged changes before testing HEAD.' }
    $head = & $git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read the commit.' }
    New-Item -ItemType Directory -Path $copy | Out-Null
    $archive = Join-Path $copy 'source.tar'
    & $git archive --format=tar --output=$archive $head
    if ($LASTEXITCODE -ne 0) { throw 'Cannot create the source archive.' }
    & $python -c 'import sys,tarfile;tarfile.open(sys.argv[1]).extractall(sys.argv[2],filter=tarfile.data_filter)' $archive $copy
    if ($LASTEXITCODE -ne 0) { throw 'Cannot extract the source archive.' }
    Remove-Item -LiteralPath $archive
    $isolatedProfile = Join-Path $copy 'isolated-user'
    $isolatedTemp = Join-Path $copy 'isolated-temp'
    if ((Test-Path -LiteralPath $isolatedProfile) -or (Test-Path -LiteralPath $isolatedTemp)) {
        throw 'Snapshot contains an isolation directory; refusing to reuse it.'
    }
    foreach ($path in @($isolatedProfile, $isolatedTemp, (Join-Path $isolatedProfile 'AppData/Roaming'), (Join-Path $isolatedProfile 'AppData/Local'))) {
        New-Item -ItemType Directory -Path $path -Force | Out-Null
    }
    $env:USERPROFILE = $isolatedProfile
    $env:APPDATA = Join-Path $isolatedProfile 'AppData/Roaming'
    $env:LOCALAPPDATA = Join-Path $isolatedProfile 'AppData/Local'
    $env:TEMP = $isolatedTemp
    $env:TMP = $isolatedTemp
    $env:GIT_CONFIG_NOSYSTEM = '1'
    $env:GIT_CONFIG_GLOBAL = Join-Path $isolatedProfile 'empty-gitconfig'
    $env:CARGO_HOME = $cargoCache
    $env:RUSTUP_HOME = $rustupCache
    $env:CARGO_TARGET_DIR = Join-Path $copy 'target'
    $env:CARGO_PROFILE_DEV_DEBUG = '0'
    $env:CARGO_PROFILE_TEST_DEBUG = '0'
    $env:CARGO_INCREMENTAL = '0'
    $env:CARGO_BUILD_JOBS = '1'
    $env:MOMSO_TEST_FAKE_CREDENTIAL_STORE = '1'
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
    Write-Host "Commit: $head"
    Write-Host "Clean copy and target: $copy"
    Push-Location $copy
    try {
        while ($true) {
            $running = @(Get-CimInstance Win32_Process -Filter "Name='cargo.exe'")
            $ids = @($running | ForEach-Object { $_.ProcessId })
            $topLevel = @($running | Where-Object { $_.ParentProcessId -notin $ids })
            $os = Get-CimInstance Win32_OperatingSystem
            $disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
            if ($disk.FreeSpace / 1GB -lt 60) { throw 'Insufficient disk space before cargo.' }
            if ($topLevel.Count -lt 2 -and $os.FreePhysicalMemory / 1MB -ge 4) { break }
            Write-Host 'Waiting for a cargo slot and available memory.'
            Start-Sleep -Seconds 60
        }
        & $cargo test --locked --manifest-path 'apps/alpha/desktop/src-tauri/Cargo.toml' -j 1 -- --test-threads=2
        if ($LASTEXITCODE -ne 0) { throw "Cargo tests failed with exit code $LASTEXITCODE." }
    } finally { Pop-Location }
} finally { Pop-Location }
