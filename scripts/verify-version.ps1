[CmdletBinding()]
param(
    [string]$ExpectedTag
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$frontendPackagePath = Join-Path $repoRoot 'frontend/package.json'
$frontendLockPath = Join-Path $repoRoot 'frontend/package-lock.json'
$backendManifestPath = Join-Path $repoRoot 'backend/Cargo.toml'

$frontendPackage = Get-Content -Raw -LiteralPath $frontendPackagePath | ConvertFrom-Json
$frontendLock = Get-Content -Raw -LiteralPath $frontendLockPath | ConvertFrom-Json -AsHashtable
$metadataJson = & cargo metadata --locked --no-deps --format-version 1 --manifest-path $backendManifestPath
if ($LASTEXITCODE -ne 0) {
    throw 'cargo metadata failed; Cargo.lock may be out of date.'
}

$metadata = $metadataJson | ConvertFrom-Json
$backendPackage = $metadata.packages | Where-Object { $_.name -eq 'crewdock-api' }
if ($null -eq $backendPackage) {
    throw 'crewdock-api was not found in Cargo metadata.'
}

$versions = [ordered]@{
    'frontend/package.json' = [string]$frontendPackage.version
    'frontend/package-lock.json top-level' = [string]$frontendLock['version']
    'frontend/package-lock.json root package' = [string]$frontendLock['packages']['']['version']
    'backend/Cargo.toml + Cargo.lock' = [string]$backendPackage.version
}
$distinctVersions = @($versions.Values | Select-Object -Unique)
if ($distinctVersions.Count -ne 1) {
    $details = ($versions.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ', '
    throw "Product versions do not match: $details"
}

$version = $distinctVersions[0]
if ($version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Product version must use release SemVer X.Y.Z, got $version."
}
$releaseTag = "v$version"
if ($ExpectedTag -and $ExpectedTag -ne $releaseTag) {
    throw "Expected tag $ExpectedTag does not match product version $releaseTag."
}

Write-Host "crewdock version check passed: $releaseTag"
