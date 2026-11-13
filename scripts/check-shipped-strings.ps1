# Packaged-exe stale-name scan. Release packaging runs this on the exe extracted
# from the shipped zip, so a binary built before a source rename cannot publish.
#
# The source tree already has `tests/naming_cleanup.rs`; this is the artifact-side
# twin. It reads the bytes as Latin-1 so ASCII/UTF-8 Rust strings stay visible, and
# it checks only user-facing phrases, not internal `SE-` container names.

param(
  [Parameter(Mandatory = $true)][string]$ExePath
)

$ErrorActionPreference = 'Continue'
if (-not (Test-Path -LiteralPath $ExePath)) {
  Write-Output ('FAIL packaged exe not found: ' + $ExePath)
  exit 1
}

$text = [Text.Encoding]::GetEncoding(28591).GetString(
  [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $ExePath).Path)
)
$oldStem = 'Simple' + 'Har'
$checks = @(
  @{ Name = 'SE is a loopback tool'; Pattern = 'SE is a loopback tool' },
  @{ Name = 'SE control token'; Pattern = 'SE control token' },
  @{ Name = 'old product stem without the final n'; Pattern = '\b' + $oldStem + '(?!n)' }
)

$failed = $false
foreach ($check in $checks) {
  if ($text -match $check.Pattern) {
    Write-Output ('FAIL packaged exe carries stale name: ' + $check.Name)
    $failed = $true
  } else {
    Write-Output ('PASS packaged exe carries no stale name: ' + $check.Name)
  }
}
if ($failed) { exit 1 }
exit 0
