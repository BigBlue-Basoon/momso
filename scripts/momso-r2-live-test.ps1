$ErrorActionPreference = 'Stop'
$freeMemoryGb = (Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB
$freeDiskGb = (Get-PSDrive C).Free / 1GB
if ($freeMemoryGb -lt 4 -or $freeDiskGb -lt 60) {
    throw 'Not enough free memory or disk space to start the development build.'
}
$workspace = Split-Path -Parent $PSScriptRoot
$oldTestRoot = $env:MOMSO_R2_TEST_ROOT
$oldTestFolder = $env:MOMSO_R2_TEST_FOLDER
$oldTestBucket = $env:MOMSO_R2_TEST_ALLOWED_BUCKET
$oldTarget = $env:CARGO_TARGET_DIR
$oldJobs = $env:CARGO_BUILD_JOBS
$oldDevDebug = $env:CARGO_PROFILE_DEV_DEBUG
$oldTestDebug = $env:CARGO_PROFILE_TEST_DEBUG
$oldIncremental = $env:CARGO_INCREMENTAL
$env:MOMSO_R2_TEST_FOLDER = '.momso-r2-live-basoon'
$env:MOMSO_R2_TEST_ALLOWED_BUCKET = 'momso-basoon'
$env:MOMSO_R2_TEST_ROOT = Join-Path $workspace $env:MOMSO_R2_TEST_FOLDER
$env:CARGO_TARGET_DIR = Join-Path $workspace 'target'
$env:CARGO_BUILD_JOBS = '1'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
Push-Location (Join-Path $workspace 'apps/alpha/desktop')
try {
    npm run tauri -- dev --config src-tauri/tauri.r2-live-test.conf.json
    if ($LASTEXITCODE -ne 0) { throw "Development app exited with code $LASTEXITCODE" }
} finally {
    Pop-Location
    $env:MOMSO_R2_TEST_ROOT = $oldTestRoot
    $env:MOMSO_R2_TEST_FOLDER = $oldTestFolder
    $env:MOMSO_R2_TEST_ALLOWED_BUCKET = $oldTestBucket
    $env:CARGO_TARGET_DIR = $oldTarget
    $env:CARGO_BUILD_JOBS = $oldJobs
    $env:CARGO_PROFILE_DEV_DEBUG = $oldDevDebug
    $env:CARGO_PROFILE_TEST_DEBUG = $oldTestDebug
    $env:CARGO_INCREMENTAL = $oldIncremental
}
