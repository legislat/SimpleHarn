# Render a release body from the checked-in changelog plus the published hashes.
# The body is generated, never hand-pasted, so table escaping and a literal
# `$linux` placeholder cannot recur.
#
# Usage:
#   ./scripts/render-release-notes.ps1 -Version v1.5.1.6 -Commit 0f8be01 `
#     -SumsFile out/v1.5.1.6/SHA256SUMS.txt `
#     -LinuxSumsFile out/v1.5.1.6/SimpleHarn-1.5.1.6-linux-x64.tar.gz.sha256 `
#     -OutFile out/v1.5.1.6/notes.md

param(
  [Parameter(Mandatory = $true)][string]$Version,
  [Parameter(Mandatory = $true)][string]$Commit,
  [Parameter(Mandatory = $true)][string]$SumsFile,
  [Parameter(Mandatory = $true)][string]$OutFile,
  [string]$LinuxSumsFile,
  [string]$Changelog = 'CHANGELOG.md'
)

$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^v\d+\.\d+\.\d+(?:\.\d+)?$') {
  throw ('version names a tag: ' + $Version)
}
$assetVersion = $Version -replace '^v', ''
if (-not (Test-Path -LiteralPath $Changelog)) { throw ('missing changelog: ' + $Changelog) }
if (-not (Test-Path -LiteralPath $SumsFile)) { throw ('missing sums: ' + $SumsFile) }

$text = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $Changelog).Path)
$heading = '(?ms)^##\s+' + [regex]::Escape($Version) + '\s+-\s+.*?(?=^##\s+|\z)'
$match = [regex]::Match($text, $heading)
if (-not $match.Success) {
  throw ('changelog has no dated entry for ' + $Version)
}
$entry = $match.Value.TrimEnd()
# Drop the heading itself; the release title carries the tag.
$body = ($entry -split "`n", 2)[1].Trim()

function Read-Sums([string]$path) {
  $rows = @()
  foreach ($line in Get-Content -LiteralPath $path) {
    if ($line -match '^([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$') {
      $rows += [pscustomobject]@{ Hash = $Matches[1].ToUpperInvariant(); Name = $Matches[2] }
    }
  }
  return $rows
}

$rows = @(Read-Sums $SumsFile)
if ($rows.Count -eq 0) { throw ('sums file has no hashes: ' + $SumsFile) }
if (-not [string]::IsNullOrWhiteSpace($LinuxSumsFile)) {
  if (-not (Test-Path -LiteralPath $LinuxSumsFile)) {
    throw ('missing Linux sums: ' + $LinuxSumsFile)
  }
  $rows += @(Read-Sums $LinuxSumsFile)
}

$table = @('## Artifacts', '', '| File | SHA256 |', '|---|---|')
foreach ($row in $rows) {
  $table += ('| ' + $row.Name + ' | ' + $row.Hash + ' |')
}
$table += ''
$table += ('Tag `' + $Version + '`, commit `' + $Commit + '`. Both binaries are built from')
$table += 'that tag; the Linux artifact needs glibc 2.16 or newer.'

$out = @(
  ('SimpleHarn ' + $assetVersion),
  '',
  $body,
  '',
  ($table -join "`n")
) -join "`n"

if ($out -match '\$linux' -or $out -match '\\\|' -or $out -match '[\x00-\x08\x0B\x0C\x0E-\x1F]') {
  throw 'rendered notes carry a placeholder, escaped cell, or control character'
}

$dir = Split-Path -Parent $OutFile
if (-not [string]::IsNullOrWhiteSpace($dir)) {
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
[IO.File]::WriteAllText($OutFile, ($out + "`n"), $utf8)
Write-Output ('PASS rendered release notes: ' + $OutFile)
