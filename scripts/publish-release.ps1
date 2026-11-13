# The only supported publish path for a SimpleHarn release.
#
# What it enforces before anything becomes public:
#   - the tag grammar and a clean release asset set;
#   - every archive asset is covered by SHA256SUMS.txt or its .sha256 companion,
#     and the recorded digest equals the file;
#   - the Windows zip carries exactly the five-entry manifest;
#   - the packaged exe reports the tag through `--version`;
#   - the cadence rule: seven days between normal releases, 24 hours between
#     hotfixes, an urgent reason for every four-segment tag, and an explicit
#     -ForceCadence override for a security emergency.
#
# Modes:
#   -Publish        create the GitHub release from -AssetDir and -NotesFile.
#   -BackfillSums   download an existing release, hash its assets, upload
#                   SHA256SUMS.txt, and append the addendum line.
#   -DryRun         validate and print the plan; no network, no publish.
#   -SkipCadence    operator-only diagnostic bypass. It prints a loud INFO and
#                   must not be used for a real release.
#
# Examples:
#   ./scripts/publish-release.ps1 -Version v1.5.1.6 -AssetDir out/v1.5.1.6 `
#     -NotesFile out/v1.5.1.6.md -Publish
#   ./scripts/publish-release.ps1 -Version v1.5.1.5 -BackfillSums -Publish
#   ./scripts/publish-release.ps1 -Version v1.5.1.6 -AssetDir out -DryRun

param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$AssetDir,
  [string]$NotesFile,
  [string]$UrgentReason,
  [switch]$BackfillSums,
  [switch]$ForceCadence,
  [switch]$DryRun,
  [switch]$Publish,
  [switch]$SkipCadence
)

$ErrorActionPreference = 'Continue'
$script:allPass = $true
function Report([bool]$ok, [string]$message) {
  if ($ok) { Write-Output ("PASS " + $message) }
  else { Write-Output ("FAIL " + $message); $script:allPass = $false }
}

$tagOk = $Version -match '^v\d+\.\d+\.\d+(?:\.\d+)?$'
Report $tagOk ("version names a tag: " + $Version)
if (-not $tagOk) { Write-Output 'PUBLISH FAIL (tag)'; exit 1 }
$assetVersion = $Version -replace '^v', ''
$segments = @($assetVersion -split '\.')
$isHotfix = ($segments.Count -eq 4)

if (-not $DryRun -and -not $Publish) {
  Write-Output 'PUBLISH FAIL (-Publish or -DryRun is required)'
  exit 1
}

function Get-Sha256([string]$path) {
  (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToUpperInvariant()
}

function Read-Sums([string]$path) {
  $map = @{}
  if (-not (Test-Path -LiteralPath $path)) { return $map }
  foreach ($line in Get-Content -LiteralPath $path) {
    if ($line -match '^([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$') {
      $map[$Matches[2]] = $Matches[1].ToUpperInvariant()
    }
  }
  return $map
}

function Test-AssetCoverage([string]$dir) {
  $sumsPath = Join-Path $dir 'SHA256SUMS.txt'
  Report (Test-Path -LiteralPath $sumsPath) 'SHA256SUMS.txt is present'
  $sums = Read-Sums $sumsPath
  $assets = @(Get-ChildItem -LiteralPath $dir -File | Where-Object {
    $_.Name -match '\.(zip|tar\.gz|exe)$'
  })
  Report ($assets.Count -gt 0) ("asset directory carries at least one archive: " + $dir)
  $linux = @($assets | Where-Object { $_.Name -like '*.tar.gz' })
  Report ($linux.Count -ge 1) 'release carries the Linux tar.gz'
  foreach ($tar in $linux) {
    Report (Test-Path -LiteralPath ($tar.FullName + '.sha256')) `
      ('Linux tar has its .sha256 companion: ' + $tar.Name)
  }
  foreach ($asset in $assets) {
    $digest = Get-Sha256 $asset.FullName
    $recorded = $null
    if ($sums.ContainsKey($asset.Name)) { $recorded = $sums[$asset.Name] }
    $companion = $asset.FullName + '.sha256'
    if ($null -eq $recorded -and (Test-Path -LiteralPath $companion)) {
      $companionSums = Read-Sums $companion
      if ($companionSums.ContainsKey($asset.Name)) { $recorded = $companionSums[$asset.Name] }
      if ($null -eq $recorded -and $companionSums.Count -eq 1) {
        $recorded = @($companionSums.Values)[0]
      }
    }
    Report ($null -ne $recorded) ("archive has a checksum: " + $asset.Name)
    if ($null -ne $recorded) {
      Report ($recorded -eq $digest) ("checksum matches the bytes: " + $asset.Name)
    }
  }
}

function Test-ZipManifest([string]$zipPath, [string]$tmpRoot) {
  $expected = @(
    'simpleharn.exe',
    'SimpleHarn.vbs',
    'README.md',
    'LICENSE',
    'docs/assets/control-page.png'
  )
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
  try {
    $entries = @($zip.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
  } finally {
    $zip.Dispose()
  }
  $ok = ($entries.Count -eq $expected.Count)
  foreach ($name in $expected) { if ($entries -notcontains $name) { $ok = $false } }
  Report $ok ("Windows zip carries exactly the five-entry manifest: " + (Split-Path $zipPath -Leaf))
  if (-not $ok) { return }
  $extract = Join-Path $tmpRoot ('manifest-' + [guid]::NewGuid().ToString('N'))
  [System.IO.Compression.ZipFile]::ExtractToDirectory($zipPath, $extract)
  $exe = Join-Path $extract 'simpleharn.exe'
  if (Test-Path -LiteralPath $exe) {
    $report = (& $exe --version 2>&1 | Out-String)
    Report ($report -match ('(?m)^release ' + [regex]::Escape($assetVersion) + '\s*$')) `
      ("packaged exe reports release " + $assetVersion)
    if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'check-shipped-strings.ps1')) {
      & (Join-Path $PSScriptRoot 'check-shipped-strings.ps1') -ExePath $exe
      Report ($LASTEXITCODE -eq 0) ('packaged exe carries no stale product strings')
    }
  } else {
    Report $false 'packaged exe is present in the zip'
  }
}

function Get-PreviousReleases() {
  if ($SkipCadence) { return @() }
  $gh = Get-Command gh -ErrorAction SilentlyContinue
  if (-not $gh) {
    Report $false 'gh is required for the cadence check (or pass -SkipCadence)'
    return @()
  }
  $json = & gh release list --limit 30 --json tagName,publishedAt 2>&1
  if ($LASTEXITCODE -ne 0) {
    Report $false ('gh release list failed: ' + ($json | Out-String).Trim())
    return @()
  }
  try {
    return @($json | ConvertFrom-Json)
  } catch {
    Report $false ('gh release list returned invalid JSON: ' + $_.Exception.Message)
    return @()
  }
}

function Test-Cadence() {
  if ($SkipCadence) {
    Write-Output 'INFO cadence check skipped by -SkipCadence'
    return
  }
  $releases = @(Get-PreviousReleases)
  if ($releases.Count -eq 0) { return }
  $now = [DateTimeOffset]::UtcNow
  $parsed = @()
  foreach ($release in $releases) {
    if ($release.tagName -eq $Version) { continue }
    try { $when = [DateTimeOffset]::Parse($release.publishedAt) } catch { continue }
    $parsed += [pscustomobject]@{ Tag = $release.tagName; When = $when }
  }
  if ($parsed.Count -eq 0) { return }
  $lastAny = ($parsed | Sort-Object When -Descending | Select-Object -First 1)
  $normals = @($parsed | Where-Object { (@($_.Tag -replace '^v', '') -split '\.').Count -eq 3 })
  $lastNormal = if ($normals.Count -gt 0) {
    ($normals | Sort-Object When -Descending | Select-Object -First 1)
  } else { $null }
  $sinceAny = $now - $lastAny.When
  Report ($sinceAny.TotalHours -ge 24) `
    ("at least 24 hours since the last release: " + $lastAny.Tag)
  if (-not $isHotfix) {
    if ($null -ne $lastNormal) {
      Report ((($now - $lastNormal.When).TotalDays -ge 7)) `
        ("normal releases are at least seven days apart (last " + $lastNormal.Tag + ')')
    }
    Report (-not ($sinceAny.TotalHours -lt 24)) 'normal release is not inside a hotfix cooldown'
  } else {
    Report (-not [string]::IsNullOrWhiteSpace($UrgentReason)) `
      'four-segment hotfix carries -UrgentReason'
    if ($sinceAny.TotalHours -lt 24) {
      Report ($ForceCadence -and -not [string]::IsNullOrWhiteSpace($UrgentReason)) `
        'inside 24 hours requires -ForceCadence and a recorded urgent reason'
    }
  }
}

function Invoke-Backfill([string]$tag) {
  $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('simpleharn-backfill-' + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $tmp | Out-Null
  $gh = Get-Command gh -ErrorAction SilentlyContinue
  if (-not $gh) {
    Report $false 'gh is required for -BackfillSums'
    return
  }
  if (-not $DryRun) {
    & gh release download $tag --dir $tmp --clobber
    Report ($LASTEXITCODE -eq 0) ('downloaded the assets for ' + $tag)
    if ($LASTEXITCODE -ne 0) { return }
  } else {
    Write-Output ('DRY RUN would download ' + $tag + ' to ' + $tmp)
    return
  }
  $sumsPath = Join-Path $tmp 'SHA256SUMS.txt'
  $lines = @()
  foreach ($asset in @(Get-ChildItem -LiteralPath $tmp -File | Where-Object {
    $_.Name -ne 'SHA256SUMS.txt' -and $_.Name -ne 'build-provenance.txt' -and $_.Extension -ne '.sha256'
  })) {
    $lines += ((Get-Sha256 $asset.FullName) + '  ' + $asset.Name)
  }
  if ($lines.Count -eq 0) {
    Report $false ('no assets to hash for ' + $tag)
    return
  }
  [IO.File]::WriteAllText($sumsPath, (($lines -join "`n") + "`n"))
  Report $true ('wrote SHA256SUMS.txt for ' + $tag)
  & gh release upload $tag $sumsPath --clobber
  Report ($LASTEXITCODE -eq 0) ('uploaded SHA256SUMS.txt for ' + $tag)
  $body = & gh release view $tag --json body --jq '.body' 2>&1
  if ($LASTEXITCODE -eq 0) {
    $today = [DateTimeOffset]::UtcNow.ToString('yyyy-MM-dd')
    $addendum = 'SHA256SUMS.txt added ' + $today + '; artifacts unchanged.'
    if (($body | Out-String) -notmatch [regex]::Escape($addendum)) {
      $notes = Join-Path $tmp 'notes.md'
      [IO.File]::WriteAllText($notes, (($body | Out-String).TrimEnd() + "`n`n" + $addendum + "`n"))
      & gh release edit $tag --notes-file $notes
      Report ($LASTEXITCODE -eq 0) ('appended the sums addendum to ' + $tag)
    }
  }
}

Write-Output ('INFO tag=' + $Version + ' hotfix=' + $isHotfix)

if ($BackfillSums) {
  Invoke-Backfill $Version
} else {
  if ([string]::IsNullOrWhiteSpace($AssetDir)) {
    Report $false '-AssetDir is required unless -BackfillSums is used'
  } elseif (-not (Test-Path -LiteralPath $AssetDir)) {
    Report $false ('asset directory does not exist: ' + $AssetDir)
  } else {
    $dir = (Resolve-Path -LiteralPath $AssetDir).Path
    $tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('simpleharn-publish-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmpRoot | Out-Null
    Test-AssetCoverage $dir
    $zip = Get-ChildItem -LiteralPath $dir -File -Filter '*-win-x64.zip' | Select-Object -First 1
    if ($null -eq $zip) { Report $false 'Windows zip is present' }
    else { Test-ZipManifest $zip.FullName $tmpRoot }
    $notesOk = $true
    if ([string]::IsNullOrWhiteSpace($NotesFile)) {
      Report $false '-NotesFile is required for a publish'
      $notesOk = $false
    } elseif (-not (Test-Path -LiteralPath $NotesFile)) {
      Report $false ('notes file does not exist: ' + $NotesFile)
      $notesOk = $false
    } else {
      $notesText = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $NotesFile).Path)
      $bad = ($notesText -match '\$linux' -or $notesText -match '\\\|' -or
        $notesText -match '[\x00-\x08\x0B\x0C\x0E-\x1F]')
      Report (-not $bad) 'notes contain no placeholder, escaped cell, or control character'
      $notesOk = -not $bad
    }
    Test-Cadence
    if ($DryRun) {
      Write-Output ('DRY RUN would publish ' + $Version + ' from ' + $dir)
    } elseif ($script:allPass -and $notesOk) {
      $assets = @(Get-ChildItem -LiteralPath $dir -File | Where-Object {
        $_.Name -eq 'SHA256SUMS.txt' -or $_.Name -match '\.(zip|tar\.gz|sha256)$'
      } | ForEach-Object { $_.FullName })
      $publishNotes = $NotesFile
      if (-not [string]::IsNullOrWhiteSpace($UrgentReason)) {
        $publishNotes = Join-Path $tmpRoot 'notes-urgent.md'
        $body = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $NotesFile).Path).TrimEnd()
        [IO.File]::WriteAllText(
          $publishNotes,
          ($body + "`n`nUrgency: hotfix - " + $UrgentReason + "`n")
        )
      }
      & gh release create $Version @assets --title ('SimpleHarn ' + $assetVersion) --notes-file $publishNotes
      Report ($LASTEXITCODE -eq 0) ('published ' + $Version)
    }
  }
}

if (-not $script:allPass) {
  Write-Output 'PUBLISH FAIL'
  exit 1
}
Write-Output 'PUBLISH PASS'
exit 0
